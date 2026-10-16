//! System integration: the one place (with `capture` and `audio`, which arrive in Phase 2) where OS-specific
//! code is allowed. Everything outside these modules talks to the OS only through the traits defined here.
//!
//! Capability detection reports what this machine can really do (SPEC Section 5). Phase 1 can detect RAM and CPU
//! cores. Everything else is reported as `NotYetImplemented` -- a capability is never guessed or faked.

pub mod disk;
mod hardware;
pub mod process;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!("MeetingMind supports Windows and macOS only (SPEC Section 3).");

use serde::Serialize;

use hardware::Hardware;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityId {
    Ram,
    CpuCores,
    SystemAudioCapture,
    CaptureExclusion,
    GpuBackend,
    VideoEncoders,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityStatus {
    /// Checked on this machine and found.
    Detected,
    // An `Unavailable` status ("checked and not there; `detail` says why") is added in Phase 2, with the first
    // check that can report it. Until then no code path could produce it.
    /// The check ran but gave no usable answer.
    Unknown,
    /// This build does not check for it yet.
    NotYetImplemented,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    pub id: CapabilityId,
    pub status: CapabilityStatus,
    /// Human-readable detected value, for example "15.7 GiB". Only present when `status` is `Detected`.
    pub value: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityReport {
    pub platform: &'static str,
    pub capabilities: Vec<Capability>,
}

/// Implemented once per supported OS. Phase 2 adds the real checks (system-audio loopback, capture exclusion,
/// encoders) inside the platform implementations.
pub trait CapabilityProbe: Send + Sync {
    fn probe(&self) -> CapabilityReport;
}

pub fn platform_probe() -> Box<dyn CapabilityProbe> {
    #[cfg(windows)]
    return Box::new(windows::WindowsProbe);
    #[cfg(target_os = "macos")]
    return Box::new(macos::MacProbe);
}

/// Builds the report for the checks every platform shares. Pure, so it can be tested with any hardware values.
fn report_from_hardware(platform: &'static str, hardware: &Hardware) -> CapabilityReport {
    CapabilityReport {
        platform,
        capabilities: vec![
            ram_capability(hardware),
            cpu_capability(hardware),
            not_yet_implemented(
                CapabilityId::SystemAudioCapture,
                "Whether system audio can be captured is not checked yet (planned for Phase 2).",
            ),
            not_yet_implemented(
                CapabilityId::CaptureExclusion,
                "Whether MeetingMind's own windows can be hidden from recordings is not checked yet (planned for Phase 2).",
            ),
            not_yet_implemented(
                CapabilityId::GpuBackend,
                "GPU backend and video memory are not detected yet (planned for Phase 3).",
            ),
            not_yet_implemented(
                CapabilityId::VideoEncoders,
                "Available video encoders are not detected yet (planned for Phase 2).",
            ),
        ],
    }
}

fn ram_capability(hardware: &Hardware) -> Capability {
    if hardware.total_memory_bytes == 0 {
        return Capability {
            id: CapabilityId::Ram,
            status: CapabilityStatus::Unknown,
            value: None,
            detail: "The operating system did not report the installed memory.".to_string(),
        };
    }
    let gib = hardware.total_memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    Capability {
        id: CapabilityId::Ram,
        status: CapabilityStatus::Detected,
        value: Some(format!("{gib:.1} GiB")),
        detail: "Total installed memory.".to_string(),
    }
}

fn cpu_capability(hardware: &Hardware) -> Capability {
    if hardware.logical_cores == 0 {
        return Capability {
            id: CapabilityId::CpuCores,
            status: CapabilityStatus::Unknown,
            value: None,
            detail: "The operating system did not report the processor core count.".to_string(),
        };
    }
    let value = match hardware.physical_cores {
        Some(physical) => format!("{} threads ({physical} cores)", hardware.logical_cores),
        None => format!("{} threads", hardware.logical_cores),
    };
    Capability {
        id: CapabilityId::CpuCores,
        status: CapabilityStatus::Detected,
        value: Some(value),
        detail: "Processor threads available to MeetingMind.".to_string(),
    }
}

fn not_yet_implemented(id: CapabilityId, detail: &str) -> Capability {
    Capability {
        id,
        status: CapabilityStatus::NotYetImplemented,
        value: None,
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(report: &CapabilityReport, id: CapabilityId) -> &Capability {
        report
            .capabilities
            .iter()
            .find(|capability| capability.id == id)
            .unwrap_or_else(|| panic!("{id:?} missing from report"))
    }

    fn sixteen_gib_eight_threads() -> Hardware {
        Hardware {
            total_memory_bytes: 16 * 1024 * 1024 * 1024,
            logical_cores: 8,
            physical_cores: Some(6),
        }
    }

    #[test]
    fn detected_hardware_is_reported_with_values() {
        let report = report_from_hardware("test", &sixteen_gib_eight_threads());

        let ram = find(&report, CapabilityId::Ram);
        assert_eq!(ram.status, CapabilityStatus::Detected);
        assert_eq!(ram.value.as_deref(), Some("16.0 GiB"));
        let cpu = find(&report, CapabilityId::CpuCores);
        assert_eq!(cpu.status, CapabilityStatus::Detected);
        assert_eq!(cpu.value.as_deref(), Some("8 threads (6 cores)"));
    }

    #[test]
    fn missing_hardware_answers_are_unknown_not_invented() {
        let report = report_from_hardware(
            "test",
            &Hardware {
                total_memory_bytes: 0,
                logical_cores: 0,
                physical_cores: None,
            },
        );

        for id in [CapabilityId::Ram, CapabilityId::CpuCores] {
            let capability = find(&report, id);
            assert_eq!(capability.status, CapabilityStatus::Unknown, "{id:?}");
            assert_eq!(capability.value, None, "{id:?}");
        }
    }

    #[test]
    fn physical_core_count_is_optional() {
        let report = report_from_hardware(
            "test",
            &Hardware {
                physical_cores: None,
                ..sixteen_gib_eight_threads()
            },
        );

        assert_eq!(
            find(&report, CapabilityId::CpuCores).value.as_deref(),
            Some("8 threads")
        );
    }

    #[test]
    fn undetectable_capabilities_are_never_reported_as_detected() {
        let report = report_from_hardware("test", &sixteen_gib_eight_threads());

        for id in [
            CapabilityId::SystemAudioCapture,
            CapabilityId::CaptureExclusion,
            CapabilityId::GpuBackend,
            CapabilityId::VideoEncoders,
        ] {
            let capability = find(&report, id);
            assert_eq!(
                capability.status,
                CapabilityStatus::NotYetImplemented,
                "{id:?}"
            );
            assert_eq!(capability.value, None, "{id:?}");
        }
    }

    #[test]
    fn every_capability_appears_exactly_once() {
        let report = report_from_hardware("test", &sixteen_gib_eight_threads());

        let mut ids: Vec<String> = report
            .capabilities
            .iter()
            .map(|c| format!("{:?}", c.id))
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), report.capabilities.len());
        assert_eq!(ids.len(), 6);
    }

    #[test]
    fn json_shape_matches_what_the_frontend_expects() {
        let report = report_from_hardware("windows", &sixteen_gib_eight_threads());

        let json = serde_json::to_value(&report).unwrap();

        assert_eq!(json["platform"], "windows");
        assert_eq!(json["capabilities"][0]["id"], "ram");
        assert_eq!(json["capabilities"][0]["status"], "detected");
        assert_eq!(json["capabilities"][2]["id"], "system_audio_capture");
        assert_eq!(json["capabilities"][2]["status"], "notYetImplemented");
    }

    /// Runs the real probe on the machine executing the tests (no fake hardware).
    #[test]
    fn the_real_probe_sees_this_machine() {
        let report = platform_probe().probe();

        assert_eq!(
            find(&report, CapabilityId::Ram).status,
            CapabilityStatus::Detected
        );
        assert_eq!(
            find(&report, CapabilityId::CpuCores).status,
            CapabilityStatus::Detected
        );
        #[cfg(windows)]
        assert_eq!(report.platform, "windows");
        #[cfg(target_os = "macos")]
        assert_eq!(report.platform, "macos");
    }
}
