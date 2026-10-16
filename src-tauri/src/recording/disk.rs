//! Disk-space guard (SPEC Section 5): warn at a low threshold; at a critical threshold the recording stops cleanly
//! and keeps everything recorded so far.

use std::io;
use std::path::PathBuf;

use crate::system::disk::DiskSpace;

pub const DEFAULT_WARN_BELOW: u64 = 2 * 1024 * 1024 * 1024;
pub const DEFAULT_STOP_BELOW: u64 = 500 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskThresholds {
    pub warn_below: u64,
    pub stop_below: u64,
}

impl Default for DiskThresholds {
    fn default() -> Self {
        Self {
            warn_below: DEFAULT_WARN_BELOW,
            stop_below: DEFAULT_STOP_BELOW,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskEvent {
    /// Nothing to report.
    None,
    /// Free space just fell below the warning threshold. Reported once, until it recovers.
    Low { available: u64 },
    /// Free space is below the stop threshold. Reported on every check: the recording must stop.
    Critical { available: u64 },
}

pub struct DiskGuard<D: DiskSpace> {
    disk: D,
    folder: PathBuf,
    thresholds: DiskThresholds,
    warned: bool,
}

impl<D: DiskSpace> DiskGuard<D> {
    pub fn new(disk: D, folder: PathBuf, thresholds: DiskThresholds) -> Self {
        assert!(
            thresholds.stop_below <= thresholds.warn_below,
            "stop threshold must not exceed the warning threshold"
        );
        Self {
            disk,
            folder,
            thresholds,
            warned: false,
        }
    }

    /// Checks the volume. Errors mean the free space could not be read; the caller should log that and keep
    /// recording rather than stop on a failed measurement.
    pub fn check(&mut self) -> io::Result<DiskEvent> {
        let available = self.disk.available_bytes(&self.folder)?;
        Ok(self.evaluate(available))
    }

    fn evaluate(&mut self, available: u64) -> DiskEvent {
        if available < self.thresholds.stop_below {
            self.warned = true;
            return DiskEvent::Critical { available };
        }
        if available < self.thresholds.warn_below {
            if !self.warned {
                self.warned = true;
                return DiskEvent::Low { available };
            }
            return DiskEvent::None;
        }
        // Back above the warning level, with a 10% margin so a value hovering at the threshold does not repeat
        // the warning on every check.
        if available > self.thresholds.warn_below + self.thresholds.warn_below / 10 {
            self.warned = false;
        }
        DiskEvent::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;

    /// TEST DOUBLE: a disk whose free space the test sets.
    #[derive(Clone)]
    struct FakeDisk(Arc<Mutex<io::Result<u64>>>);

    impl FakeDisk {
        fn with(bytes: u64) -> Self {
            Self(Arc::new(Mutex::new(Ok(bytes))))
        }
        fn set(&self, bytes: u64) {
            *self.0.lock().unwrap() = Ok(bytes);
        }
        fn fail(&self) {
            *self.0.lock().unwrap() = Err(io::Error::other("volume gone"));
        }
    }

    impl DiskSpace for FakeDisk {
        fn available_bytes(&self, _path: &Path) -> io::Result<u64> {
            match &*self.0.lock().unwrap() {
                Ok(v) => Ok(*v),
                Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
            }
        }
    }

    fn guard(disk: &FakeDisk) -> DiskGuard<FakeDisk> {
        DiskGuard::new(
            disk.clone(),
            PathBuf::from("meetings"),
            DiskThresholds::default(),
        )
    }

    #[test]
    fn plenty_of_space_reports_nothing() {
        let disk = FakeDisk::with(50 * GIB);

        assert_eq!(guard(&disk).check().unwrap(), DiskEvent::None);
    }

    #[test]
    fn crossing_the_warning_threshold_warns_once() {
        let disk = FakeDisk::with(3 * GIB);
        let mut g = guard(&disk);
        assert_eq!(g.check().unwrap(), DiskEvent::None);

        disk.set(1900 * MIB);
        assert_eq!(
            g.check().unwrap(),
            DiskEvent::Low {
                available: 1900 * MIB
            }
        );
        disk.set(1800 * MIB);
        assert_eq!(g.check().unwrap(), DiskEvent::None, "already warned");
        assert_eq!(g.check().unwrap(), DiskEvent::None);
    }

    #[test]
    fn the_critical_threshold_is_reported_on_every_check() {
        let disk = FakeDisk::with(400 * MIB);
        let mut g = guard(&disk);

        assert_eq!(
            g.check().unwrap(),
            DiskEvent::Critical {
                available: 400 * MIB
            }
        );
        assert_eq!(
            g.check().unwrap(),
            DiskEvent::Critical {
                available: 400 * MIB
            }
        );
    }

    #[test]
    fn dropping_straight_below_critical_skips_the_warning() {
        let disk = FakeDisk::with(10 * GIB);
        let mut g = guard(&disk);
        assert_eq!(g.check().unwrap(), DiskEvent::None);

        disk.set(100 * MIB);

        assert_eq!(
            g.check().unwrap(),
            DiskEvent::Critical {
                available: 100 * MIB
            }
        );
    }

    #[test]
    fn the_thresholds_are_exclusive_boundaries() {
        let disk = FakeDisk::with(DEFAULT_WARN_BELOW);
        let mut g = guard(&disk);
        assert_eq!(
            g.check().unwrap(),
            DiskEvent::None,
            "exactly at the warning level is not below it"
        );

        disk.set(DEFAULT_STOP_BELOW);
        assert_eq!(
            g.check().unwrap(),
            DiskEvent::Low {
                available: DEFAULT_STOP_BELOW
            },
            "exactly at the stop level is a warning, not a stop"
        );
    }

    #[test]
    fn the_warning_is_rearmed_only_after_space_clearly_recovers() {
        let disk = FakeDisk::with(GIB);
        let mut g = guard(&disk);
        assert!(matches!(g.check().unwrap(), DiskEvent::Low { .. }));

        // Hovering just above the threshold does not re-arm it ...
        disk.set(2 * GIB + 50 * MIB);
        assert_eq!(g.check().unwrap(), DiskEvent::None);
        disk.set(2 * GIB - 50 * MIB);
        assert_eq!(
            g.check().unwrap(),
            DiskEvent::None,
            "no repeated warning from a value hovering at the line"
        );

        // ... but a real recovery does.
        disk.set(10 * GIB);
        assert_eq!(g.check().unwrap(), DiskEvent::None);
        disk.set(GIB);
        assert!(matches!(g.check().unwrap(), DiskEvent::Low { .. }));
    }

    #[test]
    fn a_failed_measurement_is_an_error_not_a_stop() {
        let disk = FakeDisk::with(10 * GIB);
        let mut g = guard(&disk);
        disk.fail();

        assert!(g.check().is_err());
        disk.set(10 * GIB);
        assert_eq!(g.check().unwrap(), DiskEvent::None);
    }

    #[test]
    #[should_panic(expected = "stop threshold must not exceed")]
    fn thresholds_in_the_wrong_order_are_a_bug() {
        let _ = DiskGuard::new(
            FakeDisk::with(0),
            PathBuf::from("x"),
            DiskThresholds {
                warn_below: 1,
                stop_below: 2,
            },
        );
    }
}
