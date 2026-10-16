//! RAM and CPU detection through `sysinfo`, which is cross-platform. The OS-specific probes call this.

use sysinfo::System;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hardware {
    pub total_memory_bytes: u64,
    /// Threads available to this process. 0 if the OS gave no answer.
    pub logical_cores: usize,
    pub physical_cores: Option<usize>,
}

pub fn detect() -> Hardware {
    let mut system = System::new();
    system.refresh_memory();
    Hardware {
        total_memory_bytes: system.total_memory(),
        logical_cores: std::thread::available_parallelism()
            .map(|threads| threads.get())
            .unwrap_or(0),
        physical_cores: System::physical_core_count(),
    }
}
