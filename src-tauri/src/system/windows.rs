use super::{CapabilityProbe, CapabilityReport, hardware, report_from_hardware};

/// Windows capability checks. Phase 1: RAM and CPU only. Phase 2 adds WASAPI loopback, capture exclusion
/// (`WDA_EXCLUDEFROMCAPTURE`) and encoder detection here.
pub struct WindowsProbe;

impl CapabilityProbe for WindowsProbe {
    fn probe(&self) -> CapabilityReport {
        report_from_hardware("windows", &hardware::detect())
    }
}
