//! Meeting folder names (SPEC Section 7): `YYYY-MM-DD_HHMM_Meeting-Title`.
//!
//! A title typed by a user can contain anything, so it is cleaned before it becomes part of a path: characters
//! Windows forbids are removed, spaces become dashes, reserved device names are avoided, the length is limited so
//! the full path stays well under the Windows limit, and an existing folder is never reused or overwritten.

use std::io;
use std::path::{Path, PathBuf};

pub use crate::system::LocalDateTime;

const MAX_TITLE_CHARS: usize = 60;
/// Windows' classic path limit is 260. The folder holds files such as `system-audio.flac` and temporary tracks, so
/// the folder path itself is kept under this.
const MAX_FOLDER_PATH_CHARS: usize = 220;
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("the storage folder path is too long to hold a meeting folder")]
    PathTooLong,
    #[error("could not create the meeting folder: {0}")]
    Io(#[from] io::Error),
}

/// Cleans a user-typed title for use inside a file or folder name. Never returns an empty string.
pub fn sanitize_title(title: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in title.chars() {
        // Whitespace (including tabs and newlines) separates words; it is checked before the control characters.
        if c.is_whitespace() || c == '_' {
            pending_dash = !out.is_empty();
            continue;
        }
        let forbidden =
            matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control();
        if forbidden {
            continue;
        }
        if pending_dash {
            out.push('-');
            pending_dash = false;
        }
        out.push(c);
    }
    // Windows silently drops trailing dots and spaces, which would make two different titles collide.
    let trimmed = out.trim_matches(['-', '.', ' ']);
    let mut title = limit_chars(trimmed, MAX_TITLE_CHARS);
    if title.is_empty() {
        return "Untitled".to_string();
    }
    // A reserved device name is reserved even with an extension (`CON.txt`).
    let stem = title.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        title.insert(0, '_');
    }
    title
}

/// Keeps at most `max` characters (not bytes), never cutting a character in half, and drops a dangling separator.
fn limit_chars(s: &str, max: usize) -> String {
    let cut: String = s.chars().take(max).collect();
    cut.trim_end_matches(['-', '.', ' ']).to_string()
}

/// `YYYY-MM-DD_HHMM_Title`, with the title cleaned and limited to `max_title_chars`.
fn format_name(when: &LocalDateTime, title: &str, max_title_chars: usize) -> String {
    let title = limit_chars(&sanitize_title(title), max_title_chars);
    let title = if title.is_empty() {
        "Untitled".to_string()
    } else {
        title
    };
    format!(
        "{:04}-{:02}-{:02}_{:02}{:02}_{}",
        when.year, when.month, when.day, when.hour, when.minute, title
    )
}

/// The folder name for a meeting, without any collision handling.
pub fn folder_name(when: &LocalDateTime, title: &str) -> String {
    format_name(when, title, MAX_TITLE_CHARS)
}

/// Deletes a folder and everything in it, trying again for a few seconds if Windows reports that a file is still in
/// use. That happens right after a process was killed (it releases its files a moment later) and while antivirus is
/// scanning a file that was just written. A folder that does not exist counts as deleted.
pub fn remove_folder(path: &Path) -> io::Result<()> {
    remove_folder_with(path, 25, std::time::Duration::from_millis(200))
}

pub(crate) fn remove_folder_with(
    path: &Path,
    attempts: u32,
    delay: std::time::Duration,
) -> io::Result<()> {
    let mut last = None;
    for attempt in 0..attempts.max(1) {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => last = Some(e),
        }
        if attempt + 1 < attempts {
            std::thread::sleep(delay);
        }
    }
    Err(last.expect("at least one attempt was made"))
}

/// Creates a new, empty meeting folder inside `parent` and returns its path. If the name is taken (by a folder or a
/// file), `_2`, `_3`, ... is appended. An existing folder is never reused, so earlier recordings cannot be touched.
/// The title is shortened if needed to keep the whole path within the limit.
pub fn create_meeting_folder(
    parent: &Path,
    when: &LocalDateTime,
    title: &str,
) -> Result<PathBuf, LayoutError> {
    let parent_chars = parent.to_string_lossy().chars().count();
    // path = parent + separator + name + room for a "_99" collision suffix
    let overhead = parent_chars + 1 + "YYYY-MM-DD_HHMM_".len() + 3;
    if overhead >= MAX_FOLDER_PATH_CHARS {
        // Checked before anything is created, so a refused request leaves no stray folders behind.
        return Err(LayoutError::PathTooLong);
    }
    std::fs::create_dir_all(parent)?;
    let room = (MAX_FOLDER_PATH_CHARS - overhead).min(MAX_TITLE_CHARS);
    let base = format_name(when, title, room);

    let mut attempt = 1u32;
    loop {
        let name = if attempt == 1 {
            base.clone()
        } else {
            format!("{base}_{attempt}")
        };
        let path = parent.join(&name);
        // create_dir fails if the path exists, so this is an atomic "claim this name".
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => attempt += 1,
            Err(e) => return Err(e.into()),
        }
        if attempt > 9999 {
            return Err(io::Error::other("no free meeting folder name").into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn when() -> LocalDateTime {
        LocalDateTime {
            year: 2026,
            month: 10,
            day: 16,
            hour: 9,
            minute: 5,
        }
    }

    #[test]
    fn spaces_become_dashes() {
        assert_eq!(sanitize_title("Weekly team sync"), "Weekly-team-sync");
        assert_eq!(sanitize_title("  lots   of   space  "), "lots-of-space");
        assert_eq!(sanitize_title("snake_case_title"), "snake-case-title");
    }

    #[test]
    fn characters_windows_forbids_are_removed() {
        // The space after the colon is a word break; the slash and backslash are simply removed.
        assert_eq!(
            sanitize_title(r#"Q3 <review>: "plan"/budget\2026|v2?*"#),
            "Q3-review-planbudget2026v2"
        );
        assert_eq!(
            sanitize_title("tab\there\nnewline\u{7}bell"),
            "tab-here-newlinebell"
        );
    }

    #[test]
    fn non_latin_titles_are_kept() {
        assert_eq!(sanitize_title("주간 회의 – 예산"), "주간-회의-–-예산");
        assert_eq!(sanitize_title("会議メモ"), "会議メモ");
    }

    #[test]
    fn trailing_dots_and_spaces_are_trimmed() {
        assert_eq!(sanitize_title("notes..."), "notes");
        assert_eq!(sanitize_title("notes. . "), "notes");
        assert_eq!(sanitize_title("...hidden"), "hidden");
    }

    #[test]
    fn an_empty_or_unusable_title_becomes_untitled() {
        assert_eq!(sanitize_title(""), "Untitled");
        assert_eq!(sanitize_title("   "), "Untitled");
        assert_eq!(sanitize_title(r#"<>:"/\|?*"#), "Untitled");
        assert_eq!(sanitize_title("..."), "Untitled");
    }

    #[test]
    fn reserved_device_names_are_made_safe_with_or_without_an_extension() {
        for name in [
            "CON",
            "con",
            "Prn",
            "AUX",
            "NUL",
            "COM1",
            "lpt9",
            "CON.txt",
            "nul.tar.gz",
        ] {
            let cleaned = sanitize_title(name);
            assert!(cleaned.starts_with('_'), "{name} -> {cleaned}");
        }
        assert_eq!(sanitize_title("CONFERENCE"), "CONFERENCE");
        assert_eq!(
            sanitize_title("COM10"),
            "COM10",
            "only COM1-COM9 are reserved"
        );
        assert_eq!(sanitize_title("console"), "console");
    }

    #[test]
    fn a_long_title_is_cut_at_a_character_boundary() {
        let title = "가".repeat(200); // each is 3 bytes in UTF-8
        let cleaned = sanitize_title(&title);

        assert_eq!(cleaned.chars().count(), 60);
        assert!(cleaned.chars().all(|c| c == '가'));
    }

    #[test]
    fn a_cut_never_leaves_a_dangling_dash() {
        let title = format!("{} tail", "a".repeat(59)); // the 60th character would be the dash
        let cleaned = sanitize_title(&title);

        assert_eq!(cleaned, "a".repeat(59));
    }

    #[test]
    fn the_folder_name_follows_the_spec_pattern() {
        assert_eq!(
            folder_name(&when(), "Weekly team sync"),
            "2026-10-16_0905_Weekly-team-sync"
        );
        assert_eq!(folder_name(&when(), ""), "2026-10-16_0905_Untitled");
    }

    #[test]
    fn a_folder_is_created_and_a_second_one_with_the_same_name_gets_a_suffix() {
        let dir = tempfile::tempdir().unwrap();

        let first = create_meeting_folder(dir.path(), &when(), "Standup").unwrap();
        let second = create_meeting_folder(dir.path(), &when(), "Standup").unwrap();
        let third = create_meeting_folder(dir.path(), &when(), "Standup").unwrap();

        assert!(first.is_dir() && second.is_dir() && third.is_dir());
        assert_eq!(first.file_name().unwrap(), "2026-10-16_0905_Standup");
        assert_eq!(second.file_name().unwrap(), "2026-10-16_0905_Standup_2");
        assert_eq!(third.file_name().unwrap(), "2026-10-16_0905_Standup_3");
    }

    #[test]
    fn an_existing_folder_is_never_touched() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("2026-10-16_0905_Standup");
        std::fs::create_dir(&existing).unwrap();
        std::fs::write(existing.join("recording.mp4"), b"earlier recording").unwrap();

        let created = create_meeting_folder(dir.path(), &when(), "Standup").unwrap();

        assert_ne!(created, existing);
        assert_eq!(
            std::fs::read(existing.join("recording.mp4")).unwrap(),
            b"earlier recording"
        );
        assert_eq!(
            std::fs::read_dir(&created).unwrap().count(),
            0,
            "the new folder starts empty"
        );
    }

    #[test]
    fn a_file_with_the_same_name_is_also_a_collision() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("2026-10-16_0905_Standup"), b"a file").unwrap();

        let created = create_meeting_folder(dir.path(), &when(), "Standup").unwrap();

        assert_eq!(created.file_name().unwrap(), "2026-10-16_0905_Standup_2");
    }

    #[test]
    fn the_parent_folder_is_created_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("MeetingMind").join("Meetings");

        let created = create_meeting_folder(&parent, &when(), "x").unwrap();

        assert!(created.is_dir());
    }

    #[test]
    fn the_title_is_shortened_to_keep_the_whole_path_within_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("a".repeat(100));

        let created = create_meeting_folder(&parent, &when(), &"b".repeat(60)).unwrap();

        let chars = created.to_string_lossy().chars().count();
        assert!(chars <= MAX_FOLDER_PATH_CHARS, "{chars} characters");
        assert!(created.is_dir());
    }

    #[test]
    fn a_folder_with_a_file_that_is_briefly_in_use_is_still_removed() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("meeting");
        std::fs::create_dir(&folder).unwrap();
        let held = std::fs::File::create(folder.join("track.flac")).unwrap();
        // The file stays open for a moment (on Windows an open file cannot be deleted), then is released.
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            drop(held);
        });

        remove_folder_with(&folder, 25, std::time::Duration::from_millis(100)).unwrap();

        releaser.join().unwrap();
        assert!(!folder.exists());
    }

    #[test]
    fn removing_a_folder_that_is_already_gone_is_fine() {
        let dir = tempfile::tempdir().unwrap();

        remove_folder(&dir.path().join("never-existed")).unwrap();
    }

    #[test]
    fn a_parent_that_is_already_too_long_is_refused() {
        let error =
            create_meeting_folder(&PathBuf::from("x".repeat(300)), &when(), "t").unwrap_err();

        assert!(matches!(error, LayoutError::PathTooLong));
    }
}
