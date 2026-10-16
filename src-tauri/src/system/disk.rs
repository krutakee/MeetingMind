//! Free disk space for the volume that holds a path.

use std::io;
use std::path::{Component, Path, PathBuf};

use sysinfo::Disks;

/// Reports free space. The recording code depends on this trait, so tests can use a fake disk.
pub trait DiskSpace: Send + Sync {
    /// Bytes available to this user on the volume that contains `path`.
    fn available_bytes(&self, path: &Path) -> io::Result<u64>;
}

/// The real disk, via the operating system's volume list.
pub struct SystemDiskSpace;

impl DiskSpace for SystemDiskSpace {
    fn available_bytes(&self, path: &Path) -> io::Result<u64> {
        // The folder may not exist yet (the first recording creates it): measure its nearest existing parent.
        let existing = nearest_existing(path);
        let absolute = existing.canonicalize()?;
        let disks = Disks::new_with_refreshed_list();
        let mounts: Vec<(PathBuf, u64)> = disks
            .list()
            .iter()
            .map(|disk| (disk.mount_point().to_path_buf(), disk.available_space()))
            .collect();
        best_mount(&absolute, &mounts)
            .ok_or_else(|| io::Error::other("no volume found for the storage folder"))
    }
}

fn nearest_existing(path: &Path) -> PathBuf {
    let mut current = path;
    while !current.exists() {
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent,
            _ => return PathBuf::from("."),
        }
    }
    current.to_path_buf()
}

/// Windows' `canonicalize` returns `\\?\C:\...`; the volume list says `C:\`. Compare without the prefix.
fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

fn components_lowercase(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().to_lowercase()),
            Component::Normal(n) => Some(n.to_string_lossy().to_lowercase()),
            _ => None, // root and `.` carry no name
        })
        .collect()
}

/// The free space of the volume whose mount point is the longest prefix of `path`. Case-insensitive, because
/// Windows paths are.
pub fn best_mount(path: &Path, mounts: &[(PathBuf, u64)]) -> Option<u64> {
    let target = components_lowercase(&strip_verbatim(path));
    mounts
        .iter()
        .filter_map(|(mount, available)| {
            let m = components_lowercase(&strip_verbatim(mount));
            (target.len() >= m.len() && target[..m.len()] == m[..]).then_some((m.len(), *available))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, available)| available)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mounts() -> Vec<(PathBuf, u64)> {
        vec![
            (PathBuf::from(r"C:\"), 100),
            (PathBuf::from(r"D:\"), 200),
            (PathBuf::from(r"D:\Mounted\Backup"), 300),
        ]
    }

    #[cfg(windows)]
    #[test]
    fn the_volume_with_the_longest_matching_mount_point_wins() {
        assert_eq!(
            best_mount(Path::new(r"C:\Users\shan\Meetings"), &mounts()),
            Some(100)
        );
        assert_eq!(best_mount(Path::new(r"D:\Projects"), &mounts()), Some(200));
        assert_eq!(
            best_mount(Path::new(r"D:\Mounted\Backup\2026"), &mounts()),
            Some(300)
        );
        assert_eq!(best_mount(Path::new(r"E:\Other"), &mounts()), None);
    }

    #[cfg(windows)]
    #[test]
    fn matching_ignores_case_and_the_extended_length_prefix() {
        assert_eq!(
            best_mount(Path::new(r"\\?\c:\USERS\shan"), &mounts()),
            Some(100)
        );
        assert_eq!(
            best_mount(Path::new(r"d:\mounted\backup"), &mounts()),
            Some(300)
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_name_that_only_starts_like_a_mount_does_not_match() {
        let mounts = vec![(PathBuf::from(r"C:\"), 1), (PathBuf::from(r"C:\Data"), 2)];

        assert_eq!(
            best_mount(Path::new(r"C:\Database\x"), &mounts),
            Some(1),
            "C:\\Database is not under C:\\Data"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn the_volume_with_the_longest_matching_mount_point_wins_on_unix_paths() {
        let mounts = vec![
            (PathBuf::from("/"), 1),
            (PathBuf::from("/Volumes/Backup"), 2),
        ];

        assert_eq!(best_mount(Path::new("/Users/shan"), &mounts), Some(1));
        assert_eq!(best_mount(Path::new("/Volumes/Backup/x"), &mounts), Some(2));
        assert_eq!(
            best_mount(Path::new("/Volumes/BackupOld/x"), &mounts),
            Some(1)
        );
    }

    /// Asks the real operating system about the real temporary folder (no fake disk).
    #[test]
    fn the_real_disk_reports_free_space_for_an_existing_folder() {
        let dir = tempfile::tempdir().unwrap();

        let free = SystemDiskSpace.available_bytes(dir.path()).unwrap();

        assert!(
            free > 0,
            "the volume holding the temp folder reports no free space"
        );
    }

    #[test]
    fn a_folder_that_does_not_exist_yet_is_measured_through_its_parent() {
        let dir = tempfile::tempdir().unwrap();
        let future = dir.path().join("MeetingMind").join("Meetings");

        let free = SystemDiskSpace.available_bytes(&future).unwrap();

        assert!(free > 0);
    }
}
