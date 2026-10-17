//! Is a folder one that a cloud storage program uploads?
//!
//! MeetingMind itself sends nothing anywhere, but a meeting folder inside OneDrive, Dropbox, Google Drive or iCloud
//! Drive is uploaded by that program: the video and audio of the meeting end up on someone else's servers. The user
//! should know before recording, so the Record screen and Settings warn. This is a judgement from the folder names on
//! the path (these programs all use recognisable names); it cannot see what the program is really configured to do.

use std::path::Path;

/// The cloud storage program that syncs a folder on this path, by the name the user knows it by.
pub fn synced_by(path: &Path) -> Option<&'static str> {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .find_map(|name| provider_of(&name.to_lowercase()))
}

fn provider_of(name: &str) -> Option<&'static str> {
    // "OneDrive", "OneDrive - Company", "OneDrive-Personal" (macOS keeps these under Library/CloudStorage).
    if name.starts_with("onedrive") {
        Some("OneDrive")
    } else if name.starts_with("dropbox") {
        Some("Dropbox")
    } else if name == "google drive" || name.starts_with("googledrive") {
        Some("Google Drive")
    } else if matches!(
        name,
        "icloud drive" | "iclouddrive" | "mobile documents" | "com~apple~clouddocs"
    ) {
        Some("iCloud Drive")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(path: &str) -> Option<&'static str> {
        synced_by(Path::new(path))
    }

    #[test]
    fn a_documents_folder_moved_into_onedrive_is_recognised() {
        assert_eq!(
            provider("C:/Users/shan/OneDrive - Ivionics LLC/Documents/MeetingMind/Meetings"),
            Some("OneDrive")
        );
        assert_eq!(
            provider("C:/Users/anna/OneDrive/Documents/Meetings"),
            Some("OneDrive")
        );
    }

    #[test]
    fn the_other_well_known_providers_are_recognised() {
        assert_eq!(provider("C:/Users/a/Dropbox/Meetings"), Some("Dropbox"));
        assert_eq!(provider("D:/Dropbox (Team)/Meetings"), Some("Dropbox"));
        assert_eq!(provider("G:/Google Drive/Meetings"), Some("Google Drive"));
        assert_eq!(
            provider("C:/Users/a/iCloudDrive/Meetings"),
            Some("iCloud Drive")
        );
    }

    #[test]
    fn the_macos_cloud_locations_are_recognised() {
        assert_eq!(
            provider("/Users/a/Library/CloudStorage/OneDrive-Personal/Meetings"),
            Some("OneDrive")
        );
        assert_eq!(
            provider("/Users/a/Library/CloudStorage/GoogleDrive-a@b.com/My Drive/Meetings"),
            Some("Google Drive")
        );
        assert_eq!(
            provider("/Users/a/Library/Mobile Documents/com~apple~CloudDocs/Meetings"),
            Some("iCloud Drive")
        );
    }

    #[test]
    fn ordinary_folders_are_not_flagged() {
        assert_eq!(
            provider("C:/Users/shan/Documents/MeetingMind/Meetings"),
            None
        );
        assert_eq!(provider("D:/Recordings"), None);
        assert_eq!(provider("/Users/a/Movies/Meetings"), None);
        // The name has to be the folder's own: a folder that merely mentions a provider is not one.
        assert_eq!(provider("C:/Projects/notes about onedrive/Meetings"), None);
    }

    #[test]
    fn matching_ignores_case() {
        assert_eq!(provider("C:/Users/a/ONEDRIVE/Meetings"), Some("OneDrive"));
    }

    #[cfg(windows)]
    #[test]
    fn a_real_windows_path_with_backslashes_is_recognised() {
        assert_eq!(
            provider(r"C:\Users\shan\OneDrive - Ivionics LLC\Documents\MeetingMind\Meetings"),
            Some("OneDrive")
        );
    }
}
