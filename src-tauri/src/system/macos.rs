use super::{CapabilityProbe, CapabilityReport, hardware, report_from_hardware};

/// macOS capability checks. Phase 1: RAM and CPU only. Phase 5 adds ScreenCaptureKit, permissions and Metal here.
pub struct MacProbe;

impl CapabilityProbe for MacProbe {
    fn probe(&self) -> CapabilityReport {
        report_from_hardware("macos", &hardware::detect())
    }
}
