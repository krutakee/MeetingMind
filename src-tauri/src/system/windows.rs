//! Windows: capability checks, power requests, hiding windows from capture, session lock events, local time.

use std::ffi::c_void;
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{
    PowerClearRequest, PowerCreateRequest, PowerRequestDisplayRequired, PowerRequestSystemRequired,
    PowerSetRequest,
};
use windows::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::SystemServices::POWER_REQUEST_CONTEXT_VERSION;
use windows::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetMessageW, GetWindowLongPtrW, HWND_MESSAGE, MSG, PostMessageW, PostQuitMessage,
    RegisterClassW, SetWindowDisplayAffinity, SetWindowLongPtrW, TranslateMessage,
    WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_DESTROY,
    WM_NCCREATE, WM_WTSSESSION_CHANGE, WNDCLASSW,
};
use windows::core::{PWSTR, w};

use super::{
    Capability, CapabilityId, CapabilityProbe, CapabilityReport, CapabilityStatus, LocalDateTime,
    PowerManager, SessionEvent, SystemError, hardware, replace_capability, report_from_hardware,
};

/// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` needs Windows 10 version 2004, build 19041.
const MIN_BUILD_FOR_CAPTURE_EXCLUSION: u32 = 19041;

pub struct WindowsProbe;

impl CapabilityProbe for WindowsProbe {
    fn probe(&self) -> CapabilityReport {
        let mut report = report_from_hardware("windows", &hardware::detect());
        replace_capability(&mut report, capture_exclusion_capability(build_number()));
        replace_capability(&mut report, system_audio_capability());
        report
    }
}

/// The Windows build number (for example 26200), as reported by the operating system.
fn build_number() -> Option<u32> {
    sysinfo::System::kernel_version()?.trim().parse().ok()
}

fn capture_exclusion_capability(build: Option<u32>) -> Capability {
    match build {
        Some(build) if build >= MIN_BUILD_FOR_CAPTURE_EXCLUSION => Capability {
            id: CapabilityId::CaptureExclusion,
            status: CapabilityStatus::Detected,
            value: Some(format!("Windows build {build}")),
            detail: "MeetingMind's own windows can be hidden from recordings.".to_string(),
        },
        Some(build) => Capability {
            id: CapabilityId::CaptureExclusion,
            status: CapabilityStatus::Unavailable,
            value: None,
            detail: format!(
                "Hiding MeetingMind's windows from recordings needs Windows 10 build {MIN_BUILD_FOR_CAPTURE_EXCLUSION} or newer; this is build {build}. The control window would appear in recordings: record a screen that does not show it."
            ),
        },
        None => Capability {
            id: CapabilityId::CaptureExclusion,
            status: CapabilityStatus::Unknown,
            value: None,
            detail: "The Windows build number could not be read.".to_string(),
        },
    }
}

fn system_audio_capability() -> Capability {
    match crate::audio::windows::default_speakers() {
        Ok(Some(speakers)) => Capability {
            id: CapabilityId::SystemAudioCapture,
            status: CapabilityStatus::Detected,
            value: Some(speakers.name),
            detail: "System audio can be captured from the default output device (WASAPI loopback).".to_string(),
        },
        Ok(None) => Capability {
            id: CapabilityId::SystemAudioCapture,
            status: CapabilityStatus::Unavailable,
            value: None,
            detail: "There is no audio output device, so there is no system audio to capture. Record the microphone only, or connect speakers or a headset.".to_string(),
        },
        Err(error) => Capability {
            id: CapabilityId::SystemAudioCapture,
            status: CapabilityStatus::Unknown,
            value: None,
            detail: format!("The audio devices could not be listed: {error}"),
        },
    }
}

pub fn keep_component_runtime_alive() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: increments the process-wide count of MTA users; the matching decrement is never made on purpose.
        match unsafe { CoIncrementMTAUsage() } {
            Ok(_cookie) => {}
            Err(e) => tracing::warn!(error = %e, "could not keep the COM runtime alive; recording twice in one run may fail"),
        }
    });
}

pub fn local_now() -> LocalDateTime {
    // SAFETY: GetLocalTime takes no input and returns a plain struct.
    let now = unsafe { GetLocalTime() };
    LocalDateTime {
        year: now.wYear as i32,
        month: now.wMonth as u8,
        day: now.wDay as u8,
        hour: now.wHour as u8,
        minute: now.wMinute as u8,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Keeping the machine awake

pub fn power_manager() -> Box<dyn PowerManager> {
    Box::new(WindowsPower)
}

struct WindowsPower;

/// A Windows power request. Unlike `SetThreadExecutionState` it belongs to the process, not to one thread, so it
/// lasts until it is dropped no matter which thread asked.
struct PowerRequestGuard {
    handle: HANDLE,
    display: bool,
}

// SAFETY: a power request handle is a process-wide kernel handle and may be used and closed from any thread.
unsafe impl Send for PowerRequestGuard {}

impl PowerManager for WindowsPower {
    fn keep_awake(&self, keep_display_on: bool) -> Result<Box<dyn Send>, SystemError> {
        let mut reason: Vec<u16> = "MeetingMind is recording"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let context = REASON_CONTEXT {
            Version: POWER_REQUEST_CONTEXT_VERSION,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 {
                SimpleReasonString: PWSTR(reason.as_mut_ptr()),
            },
        };
        // SAFETY: `context` and the string it points to outlive the call; the returned handle is owned by the guard.
        let handle =
            unsafe { PowerCreateRequest(&context) }.map_err(|e| SystemError::Os(e.to_string()))?;
        let mut guard = PowerRequestGuard {
            handle,
            display: false,
        };
        // SAFETY: `handle` is a valid power request handle.
        unsafe { PowerSetRequest(handle, PowerRequestSystemRequired) }
            .map_err(|e| SystemError::Os(e.to_string()))?;
        if keep_display_on {
            // SAFETY: as above.
            unsafe { PowerSetRequest(handle, PowerRequestDisplayRequired) }
                .map_err(|e| SystemError::Os(e.to_string()))?;
            guard.display = true;
        }
        Ok(Box::new(guard))
    }
}

impl Drop for PowerRequestGuard {
    fn drop(&mut self) {
        // SAFETY: the handle is valid until closed here, and each request type is cleared at most once.
        unsafe {
            let _ = PowerClearRequest(self.handle, PowerRequestSystemRequired);
            if self.display {
                let _ = PowerClearRequest(self.handle, PowerRequestDisplayRequired);
            }
            let _ = CloseHandle(self.handle);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Hiding windows from capture

pub fn set_window_excluded_from_capture(
    window: *mut c_void,
    excluded: bool,
) -> Result<(), SystemError> {
    let affinity = if excluded {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    // SAFETY: the caller passes a window handle it owns; an invalid handle makes the call fail, not crash.
    unsafe { SetWindowDisplayAffinity(HWND(window), affinity) }
        .map_err(|e| SystemError::Os(e.to_string()))
}

// ---------------------------------------------------------------------------------------------------------------
// Session lock events

type EventCallback = Box<dyn Fn(SessionEvent) + Send>;

const WTS_SESSION_LOCK: usize = 0x7;
const WTS_SESSION_UNLOCK: usize = 0x8;

/// A hidden message-only window on its own thread that receives `WM_WTSSESSION_CHANGE`.
struct SessionWatcherGuard {
    window: isize,
    thread: Option<JoinHandle<()>>,
}

pub fn watch_session(on_event: EventCallback) -> Result<Box<dyn Send>, SystemError> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<isize, String>>();
    let thread = std::thread::Builder::new()
        .name("session-watcher".into())
        .spawn(move || run_session_window(on_event, ready_tx))
        .map_err(|e| SystemError::Os(e.to_string()))?;
    match ready_rx.recv() {
        Ok(Ok(window)) => Ok(Box::new(SessionWatcherGuard {
            window,
            thread: Some(thread),
        })),
        Ok(Err(message)) => {
            let _ = thread.join();
            Err(SystemError::Os(message))
        }
        Err(_) => Err(SystemError::Os(
            "the session watcher thread ended unexpectedly".into(),
        )),
    }
}

impl Drop for SessionWatcherGuard {
    fn drop(&mut self) {
        // SAFETY: posting a message to a window handle is always safe; if the window is gone it just fails.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.window as *mut c_void)),
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

fn run_session_window(on_event: EventCallback, ready: mpsc::Sender<Result<isize, String>>) {
    // SAFETY: standard Win32 window creation and message loop on this thread. The callback is boxed twice so the
    // pointer stored in the window is thin, and is freed after the loop ends, when no more messages can arrive.
    unsafe {
        let instance = match GetModuleHandleW(None) {
            Ok(instance) => instance,
            Err(e) => return drop(ready.send(Err(e.to_string()))),
        };
        let class_name = w!("MeetingMindSessionWatcher");
        let class = WNDCLASSW {
            lpfnWndProc: Some(session_window_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&class); // fails harmlessly if the class already exists from an earlier watcher

        let state: *mut EventCallback = Box::into_raw(Box::new(on_event));
        let window = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!(""),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            Some(state as *const c_void),
        ) {
            Ok(window) => window,
            Err(e) => {
                drop(Box::from_raw(state));
                return drop(ready.send(Err(e.to_string())));
            }
        };
        if let Err(e) = WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION) {
            let _ = DestroyWindow(window);
            drop(Box::from_raw(state));
            return drop(ready.send(Err(e.to_string())));
        }
        let _ = ready.send(Ok(window.0 as isize));

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        let _ = WTSUnRegisterSessionNotification(window);
        drop(Box::from_raw(state));
    }
}

unsafe extern "system" fn session_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: called by Windows with valid arguments on the window's own thread. GWLP_USERDATA holds the callback
    // pointer stored at WM_NCCREATE and is valid until the message loop has ended.
    unsafe {
        match message {
            WM_NCCREATE => {
                let create = lparam.0 as *const CREATESTRUCTW;
                SetWindowLongPtrW(window, GWLP_USERDATA, (*create).lpCreateParams as isize);
                DefWindowProcW(window, message, wparam, lparam)
            }
            WM_WTSSESSION_CHANGE => {
                let callback = GetWindowLongPtrW(window, GWLP_USERDATA) as *const EventCallback;
                if !callback.is_null() {
                    match wparam.0 {
                        WTS_SESSION_LOCK => (*callback)(SessionEvent::Locked),
                        WTS_SESSION_UNLOCK => (*callback)(SessionEvent::Unlocked),
                        _ => {}
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;

    #[test]
    fn a_recent_windows_build_can_hide_windows_from_capture() {
        let capability = capture_exclusion_capability(Some(26200));

        assert_eq!(capability.status, CapabilityStatus::Detected);
        assert_eq!(capability.value.as_deref(), Some("Windows build 26200"));
    }

    #[test]
    fn an_old_windows_build_says_what_the_limit_is_and_what_to_do() {
        let capability = capture_exclusion_capability(Some(18363));

        assert_eq!(capability.status, CapabilityStatus::Unavailable);
        assert!(
            capability.detail.contains("19041") && capability.detail.contains("18363"),
            "{}",
            capability.detail
        );
    }

    #[test]
    fn an_unreadable_build_number_is_unknown_not_guessed() {
        assert_eq!(
            capture_exclusion_capability(None).status,
            CapabilityStatus::Unknown
        );
    }

    #[test]
    fn the_real_probe_reports_what_this_machine_can_do() {
        let report = WindowsProbe.probe();

        let status = |id| {
            report
                .capabilities
                .iter()
                .find(|c| c.id == id)
                .unwrap()
                .status
        };
        assert_ne!(
            status(CapabilityId::CaptureExclusion),
            CapabilityStatus::NotYetImplemented
        );
        assert_ne!(
            status(CapabilityId::SystemAudioCapture),
            CapabilityStatus::NotYetImplemented
        );
    }

    #[test]
    fn the_local_time_is_a_real_date() {
        let now = local_now();

        assert!(now.year >= 2026, "{now:?}");
        assert!(
            (1..=12).contains(&now.month) && (1..=31).contains(&now.day),
            "{now:?}"
        );
        assert!(now.hour < 24 && now.minute < 60, "{now:?}");
    }

    #[test]
    fn keeping_the_machine_awake_can_be_requested_and_released() {
        let manager = WindowsPower;

        let system_only = manager.keep_awake(false).expect("system request");
        let with_display = manager
            .keep_awake(true)
            .expect("system and display request");

        drop(with_display);
        drop(system_only);
    }

    /// The watcher's window receives the same message Windows sends on lock and unlock. Here the message is posted
    /// by the test (simulated), because locking the real workstation would lock the user out.
    #[test]
    fn session_messages_reach_the_callback() {
        let (tx, rx) = channel();
        let watcher = watch_session(Box::new(move |event| {
            let _ = tx.send(event);
        }))
        .expect("watcher starts");

        // Find the watcher's message-only window and post the lock and unlock notifications to it.
        // SAFETY: plain Win32 lookups and PostMessageW on a window that exists.
        unsafe {
            let window = FindWindowExW(
                Some(HWND_MESSAGE),
                None,
                w!("MeetingMindSessionWatcher"),
                None,
            )
            .expect("the watcher window exists");
            PostMessageW(
                Some(window),
                WM_WTSSESSION_CHANGE,
                WPARAM(WTS_SESSION_LOCK),
                LPARAM(0),
            )
            .unwrap();
            PostMessageW(
                Some(window),
                WM_WTSSESSION_CHANGE,
                WPARAM(WTS_SESSION_UNLOCK),
                LPARAM(0),
            )
            .unwrap();
            PostMessageW(Some(window), WM_WTSSESSION_CHANGE, WPARAM(0x1), LPARAM(0)).unwrap(); // some other change
        }

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            SessionEvent::Locked
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            SessionEvent::Unlocked
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "other session changes are ignored"
        );
        drop(watcher);
    }

    #[test]
    fn hiding_a_window_that_does_not_exist_is_an_error_not_a_crash() {
        let result = set_window_excluded_from_capture(std::ptr::null_mut(), true);

        assert!(result.is_err());
    }
}
