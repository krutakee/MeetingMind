//! Turns the irregular frames a screen source delivers into a constant-frame-rate stream.
//!
//! Windows.Graphics.Capture sends a frame only when something on screen changes (gaps of 9.5 s were measured on an
//! idle monitor), but a video file needs frame *k* at exactly `k / fps`. Output slot *k* therefore shows the newest
//! frame captured at or before `k / fps`, repeating it while the screen is still, and skipping frames when several
//! arrive within one slot. Before the first frame arrives the slot is black. The video can never run shorter or longer
//! than media time.
//!
//! Gaps (screen locked, monitor lost) are timed like frames: they are applied in order at the media time the source
//! reported, not when the worker happened to process them, so a busy worker cannot move a gap.

use std::collections::VecDeque;
use std::time::Duration;

/// What to write for one output slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot<F> {
    /// The newest captured frame (possibly repeated).
    Frame(F),
    /// Nothing has been captured yet, or the source is in a gap.
    Black,
}

enum Item<F> {
    Frame(F),
    GapStart,
    GapEnd,
}

pub struct VideoPacer<F> {
    fps: u32,
    next_slot: u64,
    /// Everything not yet applied, oldest first, with media times.
    pending: VecDeque<(Duration, Item<F>)>,
    latest: Option<F>,
    /// The frame shown before a gap began, shown again after it ends if nothing newer has arrived.
    before_gap: Option<F>,
    in_gap: bool,
}

impl<F: Clone> VideoPacer<F> {
    pub fn new(fps: u32) -> Self {
        assert!(fps > 0, "frame rate must be positive");
        Self {
            fps,
            next_slot: 0,
            pending: VecDeque::new(),
            latest: None,
            before_gap: None,
            in_gap: false,
        }
    }

    /// Media time of output slot `k`.
    fn slot_time(&self, k: u64) -> Duration {
        Duration::from_nanos(k * 1_000_000_000 / self.fps as u64)
    }

    /// Records a captured frame. Times must not decrease between calls to any of the `push`/`begin`/`end` methods.
    pub fn push_frame(&mut self, media_time: Duration, frame: F) {
        self.pending.push_back((media_time, Item::Frame(frame)));
    }

    /// The source cannot deliver video from `at` on: slots from then are black until `end_gap`.
    pub fn begin_gap(&mut self, at: Duration) {
        self.pending.push_back((at, Item::GapStart));
    }

    /// The source delivers again from `at` on.
    pub fn end_gap(&mut self, at: Duration) {
        self.pending.push_back((at, Item::GapEnd));
    }

    /// Number of slots already produced.
    pub fn slots_emitted(&self) -> u64 {
        self.next_slot
    }

    /// Produces every slot whose time is at or before `media_now`.
    pub fn advance_to(&mut self, media_now: Duration) -> Vec<Slot<F>> {
        let mut out = Vec::new();
        loop {
            let t = self.slot_time(self.next_slot);
            if t > media_now {
                break;
            }
            while let Some((at, _)) = self.pending.front() {
                if *at > t {
                    break;
                }
                let (_, item) = self.pending.pop_front().expect("front() was Some");
                match item {
                    Item::Frame(frame) => {
                        if !self.in_gap {
                            self.latest = Some(frame);
                            self.before_gap = None;
                        }
                    }
                    Item::GapStart => {
                        if !self.in_gap {
                            self.in_gap = true;
                            self.before_gap = self.latest.take();
                        }
                    }
                    Item::GapEnd => {
                        if self.in_gap {
                            self.in_gap = false;
                            if self.latest.is_none() {
                                self.latest = self.before_gap.take();
                            }
                        }
                    }
                }
            }
            out.push(match (&self.latest, self.in_gap) {
                (Some(frame), false) => Slot::Frame(frame.clone()),
                _ => Slot::Black,
            });
            self.next_slot += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    /// Frames are small integers so tests can read the output as text.
    fn show(slots: &[Slot<u32>]) -> String {
        slots
            .iter()
            .map(|slot| match slot {
                Slot::Frame(n) => n.to_string(),
                Slot::Black => "-".to_string(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn a_static_screen_repeats_the_one_frame() {
        let mut pacer = VideoPacer::new(10); // slots every 100 ms
        pacer.push_frame(ms(5), 1);

        let out = pacer.advance_to(ms(500));

        assert_eq!(
            show(&out),
            "- 1 1 1 1 1",
            "slot 0 is at 0 ms, before the frame arrived"
        );
    }

    #[test]
    fn the_video_length_always_matches_media_time() {
        let mut pacer = VideoPacer::new(15);
        pacer.push_frame(ms(0), 1);

        let out = pacer.advance_to(Duration::from_secs(60));

        assert_eq!(out.len(), 60 * 15 + 1, "slots at 0, 1/15, ... 60 s");
        assert_eq!(pacer.slots_emitted(), 901);
    }

    #[test]
    fn nothing_is_emitted_before_the_next_slot_is_due() {
        let mut pacer = VideoPacer::<u32>::new(10);

        assert_eq!(show(&pacer.advance_to(ms(0))), "-");
        assert_eq!(show(&pacer.advance_to(ms(99))), "");
        assert_eq!(show(&pacer.advance_to(ms(100))), "-");
    }

    #[test]
    fn several_frames_in_one_slot_collapse_to_the_newest() {
        let mut pacer = VideoPacer::new(10);
        pacer.push_frame(ms(10), 1);
        pacer.push_frame(ms(40), 2);
        pacer.push_frame(ms(90), 3);
        pacer.push_frame(ms(130), 4); // belongs to the next slot

        let out = pacer.advance_to(ms(200));

        assert_eq!(
            show(&out),
            "- 3 4",
            "slot 1 (100 ms) shows frame 3, slot 2 (200 ms) shows frame 4"
        );
    }

    #[test]
    fn a_frame_captured_after_a_slot_is_not_shown_in_it() {
        let mut pacer = VideoPacer::new(10);
        pacer.push_frame(ms(0), 1);
        pacer.push_frame(ms(150), 2);

        let out = pacer.advance_to(ms(200));

        assert_eq!(show(&out), "1 1 2");
    }

    #[test]
    fn a_long_stall_is_filled_with_the_last_frame() {
        let mut pacer = VideoPacer::new(10);
        pacer.push_frame(ms(0), 1);
        let _ = pacer.advance_to(ms(100));

        // Nothing for 9.5 s (as seen on an idle monitor), then a new frame.
        let during = pacer.advance_to(ms(9_500));
        pacer.push_frame(ms(9_550), 2);
        let after = pacer.advance_to(ms(9_700));

        assert!(during.iter().all(|slot| *slot == Slot::Frame(1)));
        assert_eq!(during.len(), 94, "slots at 200 ms .. 9500 ms");
        assert_eq!(
            show(&after),
            "2 2",
            "slots at 9600 and 9700 ms; the new frame was captured at 9550 ms"
        );
    }

    #[test]
    fn a_gap_makes_slots_black_from_the_time_it_began() {
        let mut pacer = VideoPacer::new(10);
        pacer.push_frame(ms(0), 1);
        pacer.begin_gap(ms(250));
        pacer.end_gap(ms(550));
        pacer.push_frame(ms(600), 2);

        let out = pacer.advance_to(ms(800));

        assert_eq!(
            show(&out),
            "1 1 1 - - - 2 2 2",
            "black at 300, 400, 500 ms; the new frame at 600 ms"
        );
    }

    #[test]
    fn a_gap_is_applied_at_its_own_time_even_when_processed_late() {
        // The worker was busy and handles everything at once, long after it happened.
        let mut pacer = VideoPacer::new(10);
        pacer.push_frame(ms(0), 1);
        pacer.begin_gap(ms(1_000));
        pacer.end_gap(ms(1_500));
        pacer.push_frame(ms(1_500), 2);

        let out = pacer.advance_to(ms(2_000));

        // Slots at 0..900 ms show frame 1 (ten slots), 1000..1400 ms are black (five), 1500..2000 ms show frame 2.
        assert_eq!(show(&out), "1 1 1 1 1 1 1 1 1 1 - - - - - 2 2 2 2 2 2");
    }

    #[test]
    fn frames_captured_during_a_gap_are_never_shown() {
        let mut pacer = VideoPacer::new(10);
        pacer.push_frame(ms(0), 1);
        pacer.begin_gap(ms(150));
        pacer.push_frame(ms(250), 99); // arrives during the gap: ignored
        pacer.end_gap(ms(450));

        let out = pacer.advance_to(ms(600));

        assert_eq!(show(&out), "1 1 - - - 1 1");
    }

    #[test]
    fn after_a_gap_with_no_new_frame_the_frame_from_before_it_is_shown_again() {
        // A still desktop after unlocking may not produce a new frame: better the old picture than black forever.
        let mut pacer = VideoPacer::new(10);
        pacer.push_frame(ms(0), 7);
        pacer.begin_gap(ms(150));
        pacer.end_gap(ms(350));

        let out = pacer.advance_to(ms(600));

        assert_eq!(show(&out), "7 7 - - 7 7 7");
    }

    #[test]
    fn slot_times_do_not_accumulate_rounding_error() {
        // 15 fps is not a whole number of milliseconds per frame; the 108,000th slot must still be exact.
        let pacer = VideoPacer::<u32>::new(15);

        assert_eq!(pacer.slot_time(15), Duration::from_secs(1));
        assert_eq!(pacer.slot_time(15 * 3600 * 2), Duration::from_secs(7200));
    }

    #[test]
    #[should_panic(expected = "frame rate must be positive")]
    fn a_zero_frame_rate_is_a_bug() {
        let _ = VideoPacer::<u32>::new(0);
    }
}
