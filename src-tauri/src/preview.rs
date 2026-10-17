//! The live preview on the Record screen: a small picture of the chosen monitor, refreshed a couple of times a second.
//!
//! It uses the same screen source as recording (so what is shown is what would be recorded), asked for a low frame
//! rate, and keeps only the newest frame, scaled down. Free of any UI framework: the command layer hands the frame to
//! the UI. At most one preview runs; it stops when a recording starts.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tracing::warn;

use crate::capture::{ScreenEvent, ScreenSource, VideoFrame};
use crate::recorder::Devices;
use crate::recording::clock::SystemClock;

/// Frames per second asked of the screen source.
const PREVIEW_FPS: u32 = 2;
/// The preview is scaled down to at most this many pixels wide.
pub const MAX_WIDTH: u32 = 640;

/// One preview picture: RGBA, `width * 4` bytes per row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFrame {
    /// Counts up with every new picture, so the UI can ask only for newer ones.
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Scales a captured BGRA frame down by averaging whole blocks of pixels, and turns it into RGBA. A frame already
/// narrower than `max_width` is only converted. Block averaging keeps small text readable better than skipping pixels.
pub fn shrink(frame: &VideoFrame, max_width: u32) -> (u32, u32, Vec<u8>) {
    let block = frame.width.div_ceil(max_width.max(1)).max(1);
    let (out_width, out_height) = ((frame.width / block).max(1), (frame.height / block).max(1));
    let mut rgba = Vec::with_capacity(out_width as usize * out_height as usize * 4);
    let stride = frame.width as usize * 4;
    let samples = u32::pow(block, 2);
    for oy in 0..out_height {
        for ox in 0..out_width {
            let (mut b, mut g, mut r) = (0u32, 0u32, 0u32);
            for dy in 0..block {
                let row = (oy * block + dy) as usize * stride;
                for dx in 0..block {
                    let at = row + (ox * block + dx) as usize * 4;
                    b += u32::from(frame.bgra[at]);
                    g += u32::from(frame.bgra[at + 1]);
                    r += u32::from(frame.bgra[at + 2]);
                }
            }
            rgba.extend_from_slice(&[
                (r / samples) as u8,
                (g / samples) as u8,
                (b / samples) as u8,
                255,
            ]);
        }
    }
    (out_width, out_height, rgba)
}

struct Running {
    monitor_id: Option<String>,
    source: Box<dyn ScreenSource>,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

#[derive(Default)]
pub struct Preview {
    running: Mutex<Option<Running>>,
    latest: Arc<Mutex<Option<PreviewFrame>>>,
    sequence: Arc<AtomicU64>,
}

impl Preview {
    /// Starts previewing a monitor (`None` is the primary one), or keeps going if it is already the one shown.
    /// Opening the screen takes about a second, so call it off the UI thread.
    pub fn start(&self, devices: &dyn Devices, monitor_id: Option<&str>) -> Result<(), String> {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if running
            .as_ref()
            .is_some_and(|r| r.monitor_id.as_deref() == monitor_id)
        {
            return Ok(());
        }
        if let Some(old) = running.take() {
            Self::end(old);
        }
        *self.latest.lock().unwrap_or_else(|e| e.into_inner()) = None;

        let mut source = devices
            .screen(monitor_id, PREVIEW_FPS)
            .map_err(|e| e.to_string())?;
        source.prepare().map_err(|e| e.to_string())?;
        let (tx, rx) = sync_channel::<ScreenEvent>(2);
        source
            .start(Arc::new(SystemClock::new()), tx)
            .map_err(|e| e.to_string())?;

        let stop = Arc::new(AtomicBool::new(false));
        let (latest, sequence, stop_flag) =
            (self.latest.clone(), self.sequence.clone(), stop.clone());
        let thread = std::thread::Builder::new()
            .name("preview".into())
            .spawn(move || {
                while !stop_flag.load(Ordering::SeqCst) {
                    match rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(ScreenEvent::Frame(frame)) => {
                            let (width, height, rgba) = shrink(&frame, MAX_WIDTH);
                            let sequence = sequence.fetch_add(1, Ordering::SeqCst) + 1;
                            *latest.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(PreviewFrame {
                                    sequence,
                                    width,
                                    height,
                                    rgba,
                                });
                        }
                        // A locked screen or a lost monitor: the last picture stays.
                        Ok(ScreenEvent::Lost { .. } | ScreenEvent::Restored { .. }) => {}
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        *running = Some(Running {
            monitor_id: monitor_id.map(str::to_string),
            source,
            stop,
            thread,
        });
        Ok(())
    }

    /// The newest picture if it is newer than `since` (use 0 to get any).
    pub fn latest_after(&self, since: u64) -> Option<PreviewFrame> {
        self.latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|frame| frame.sequence > since)
            .cloned()
    }

    pub fn stop(&self) {
        let old = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(old) = old {
            Self::end(old);
        }
        *self.latest.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn is_running(&self) -> bool {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    fn end(mut running: Running) {
        running.stop.store(true, Ordering::SeqCst);
        running.source.stop();
        if running.thread.join().is_err() {
            warn!("the preview thread ended with a panic");
        }
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, pixel: [u8; 4]) -> VideoFrame {
        let bgra: Vec<u8> = (0..width * height).flat_map(|_| pixel).collect();
        VideoFrame {
            captured_at: Duration::ZERO,
            width,
            height,
            bgra: Arc::new(bgra),
        }
    }

    #[test]
    fn a_full_hd_frame_becomes_640_by_360_and_blue_green_red_becomes_red_green_blue() {
        // BGRA (10, 20, 30, 255): blue 10, green 20, red 30.
        let (width, height, rgba) = shrink(&frame(1920, 1080, [10, 20, 30, 255]), MAX_WIDTH);

        assert_eq!((width, height), (640, 360));
        assert_eq!(rgba.len(), 640 * 360 * 4);
        assert_eq!(&rgba[..4], &[30, 20, 10, 255]);
    }

    #[test]
    fn a_small_frame_is_converted_but_not_scaled() {
        let (width, height, rgba) = shrink(&frame(320, 180, [0, 0, 255, 255]), MAX_WIDTH);

        assert_eq!((width, height), (320, 180));
        assert_eq!(&rgba[..4], &[255, 0, 0, 255], "a red pixel stays red");
    }

    #[test]
    fn a_block_is_averaged_not_sampled() {
        // 2 x 1 frame shrunk with a block of 2: one black and one white pixel average to grey.
        let mut bgra = vec![0u8, 0, 0, 255];
        bgra.extend_from_slice(&[255, 255, 255, 255]);
        let two_pixels = VideoFrame {
            captured_at: Duration::ZERO,
            width: 2,
            height: 2,
            bgra: Arc::new([bgra.clone(), bgra].concat()),
        };

        let (width, height, rgba) = shrink(&two_pixels, 1);

        assert_eq!((width, height), (1, 1));
        assert_eq!(&rgba[..3], &[127, 127, 127]);
    }

    #[test]
    fn odd_sizes_drop_the_partial_block_at_the_edge() {
        let (width, height, rgba) = shrink(&frame(1366, 768, [1, 2, 3, 255]), MAX_WIDTH);

        // 1366 / 3 = 455 columns, 768 / 3 = 256 rows.
        assert_eq!((width, height), (455, 256));
        assert_eq!(rgba.len(), 455 * 256 * 4);
    }
}
