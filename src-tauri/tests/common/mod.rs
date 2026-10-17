//! Helpers shared by the tests that use the real devices of the machine (hardware_smoke.rs, soak.rs): FFmpeg
//! measurements of a recording, a window of a known colour, and a quiet tone on the speakers.
#![allow(dead_code)] // each test program uses only some of these

use std::ffi::c_void;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, EndPaint, FillRect, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, MSG,
    PostMessageW, PostQuitMessage, RegisterClassW, SW_HIDE, SW_SHOWNA, ShowWindow,
    TranslateMessage, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_PAINT, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};
use windows::core::w;

use meetingmind_lib::recording::clock::SystemClock;
use meetingmind_lib::recording::ffmpeg::{self, FfmpegInfo, H264Kind};
use meetingmind_lib::recording::session::SessionDeps;
use meetingmind_lib::system::disk::SystemDiskSpace;
use meetingmind_lib::system::platform_power;

pub fn ffmpeg_info() -> (FfmpegInfo, H264Kind) {
    let info = ffmpeg::locate()
        .and_then(|p| ffmpeg::probe(&p))
        .expect("FFmpeg is installed (pnpm fetch:ffmpeg)");
    let kind = ffmpeg::select_h264(&info).expect("an H.264 encoder works");
    (info, kind)
}

pub fn assert_close(actual: Duration, expected: f64, tolerance: f64, what: &str) {
    let a = actual.as_secs_f64();
    assert!(
        (a - expected).abs() <= tolerance,
        "{what}: {a:.3} s, expected {expected:.3} s (+-{tolerance})"
    );
}

pub fn length(info: &FfmpegInfo, file: &Path) -> Duration {
    ffmpeg::measure_duration(&info.path, file)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()))
}

/// The color of one pixel of the video at time `t`, as RGB.
pub fn pixel_at(info: &FfmpegInfo, video: &Path, t: f64, x: u32, y: u32) -> [u8; 3] {
    let output = Command::new(&info.path)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            &format!("{t}"),
            "-i",
        ])
        .arg(video)
        .args([
            "-frames:v",
            "1",
            "-vf",
            // A 2x2 crop at even coordinates: video with subsampled color cannot be cropped to one pixel.
            &format!("crop=2:2:{}:{}", x & !1, y & !1),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .output()
        .expect("ffmpeg runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.len(), 12, "expected a 2x2 RGB crop at t={t}");
    [output.stdout[0], output.stdout[1], output.stdout[2]]
}

pub fn is_magenta(pixel: [u8; 3]) -> bool {
    pixel[0] > 190 && pixel[1] < 90 && pixel[2] > 190
}

/// The loudest sample of an audio file, in dB below full scale.
pub fn max_volume_db(info: &FfmpegInfo, file: &Path) -> f64 {
    let output = Command::new(&info.path)
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(file)
        .args(["-af", "volumedetect", "-f", "null", "-"])
        .output()
        .expect("ffmpeg runs");
    let text = String::from_utf8_lossy(&output.stderr);
    let line = text
        .lines()
        .find(|l| l.contains("max_volume"))
        .unwrap_or_else(|| panic!("no volume reported: {text}"));
    line.split("max_volume:")
        .nth(1)
        .unwrap()
        .trim()
        .trim_end_matches(" dB")
        .parse()
        .unwrap()
}

// --- a window with a known color, as a test prop ---------------------------------------------------------------

pub struct TestWindow {
    handle: isize,
    thread: Option<JoinHandle<()>>,
}

unsafe extern "system" fn test_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: called by Windows on the window's own thread with valid arguments.
    unsafe {
        match message {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let dc = BeginPaint(window, &mut paint);
                let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00FF00FF)); // magenta
                FillRect(dc, &paint.rcPaint, brush);
                let _ = EndPaint(window, &paint);
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(window);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

impl TestWindow {
    /// A magenta, always-on-top window of this size at this position (physical pixels), without stealing focus.
    pub fn show(x: i32, y: i32, width: i32, height: i32) -> Self {
        let (ready_tx, ready_rx) = mpsc::channel::<isize>();
        let thread = std::thread::spawn(move || {
            // SAFETY: plain Win32 window creation and message loop on this thread.
            unsafe {
                let instance = GetModuleHandleW(None).unwrap();
                let class_name = w!("MeetingMindSmokeTestWindow");
                let class = WNDCLASSW {
                    lpfnWndProc: Some(test_window_proc),
                    hInstance: instance.into(),
                    lpszClassName: class_name,
                    ..Default::default()
                };
                RegisterClassW(&class);
                let window = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    class_name,
                    w!("MeetingMind smoke test"),
                    WS_POPUP | WS_VISIBLE,
                    x,
                    y,
                    width,
                    height,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap();
                ready_tx.send(window.0 as isize).unwrap();
                let mut message = MSG::default();
                while GetMessageW(&mut message, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        });
        let handle = ready_rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(500)); // let Windows paint it
        Self {
            handle,
            thread: Some(thread),
        }
    }

    pub fn raw(&self) -> *mut c_void {
        self.handle as *mut c_void
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: posting to a window handle is always safe.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.handle as *mut c_void)),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Plays a quiet 440 Hz tone on the default speakers until dropped. The stream stays on the creating thread.
pub struct Tone {
    _stream: cpal::Stream,
}

impl Tone {
    pub fn start(amplitude: f32) -> Self {
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
                    for frame in data.chunks_mut(channels) {
                        phase = (phase + 2.0 * std::f32::consts::PI * 440.0 / rate)
                            % (2.0 * std::f32::consts::PI);
                        frame.fill(amplitude * phase.sin());
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

pub fn make_deps(info: FfmpegInfo, kind: H264Kind) -> SessionDeps {
    SessionDeps {
        clock: Arc::new(SystemClock::new()),
        ffmpeg: info,
        video_encoder: kind,
        disk: Arc::new(SystemDiskSpace),
        database: None,
        power: Some(Arc::from(platform_power())),
    }
}

/// Physical-pixel coordinates need a DPI-aware process, or Windows scales the window position.
pub fn make_process_dpi_aware() {
    // SAFETY: changes this process's DPI awareness; fails harmlessly if it was already set.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

impl TestWindow {
    /// Shows or hides the window at once, without stealing the focus: a flash on the screen.
    pub fn set_visible(&self, visible: bool) {
        // SAFETY: showing or hiding a window handle is always safe; a window that is gone just ignores it.
        unsafe {
            let _ = ShowWindow(
                HWND(self.handle as *mut c_void),
                if visible { SW_SHOWNA } else { SW_HIDE },
            );
        }
    }
}
