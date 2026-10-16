//! macOS: RAM and CPU detection and the local clock. Everything else arrives in Phase 5 and is reported honestly as
//! not supported yet.

use std::ffi::c_void;

use super::{
    CapabilityProbe, CapabilityReport, LocalDateTime, PowerManager, SessionEvent, SystemError,
    hardware, report_from_hardware,
};

/// macOS capability checks. For now: RAM and CPU only. Phase 5 adds ScreenCaptureKit, permissions and Metal here.
pub struct MacProbe;

impl CapabilityProbe for MacProbe {
    fn probe(&self) -> CapabilityReport {
        report_from_hardware("macos", &hardware::detect())
    }
}

/// Nothing to keep alive on macOS.
pub fn keep_component_runtime_alive() {}

pub fn local_now() -> LocalDateTime {
    // SAFETY: `time` and `localtime_r` are given valid pointers to local variables.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut parts: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut parts);
        LocalDateTime {
            year: parts.tm_year + 1900,
            month: (parts.tm_mon + 1) as u8,
            day: parts.tm_mday as u8,
            hour: parts.tm_hour as u8,
            minute: parts.tm_min as u8,
        }
    }
}

struct NotYetPower;

impl PowerManager for NotYetPower {
    fn keep_awake(&self, _keep_display_on: bool) -> Result<Box<dyn Send>, SystemError> {
        Err(SystemError::NotSupported(
            "keeping the Mac awake while recording (Phase 5)",
        ))
    }
}

pub fn power_manager() -> Box<dyn PowerManager> {
    Box::new(NotYetPower)
}

pub fn set_window_excluded_from_capture(
    _window: *mut c_void,
    _excluded: bool,
) -> Result<(), SystemError> {
    Err(SystemError::NotSupported(
        "hiding windows from capture (Phase 5)",
    ))
}

pub fn watch_session(
    _on_event: Box<dyn Fn(SessionEvent) + Send>,
) -> Result<Box<dyn Send>, SystemError> {
    Err(SystemError::NotSupported("screen lock events (Phase 5)"))
}
