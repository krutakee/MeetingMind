//! Named recording setups ("presets") for the Record screen, stored in `recording_presets`.
//!
//! The database keeps each setup as a JSON document and checks only that it is valid JSON; what is inside belongs to
//! `recording::setup`. A preset whose JSON can no longer be read (written by a newer build, say) is skipped when
//! listing and never makes the list fail.

use rusqlite::{Connection, params};
use serde::Serialize;
use tracing::warn;

use crate::error::AppError;
use crate::recording::setup::RecordingSetup;

/// Settings key holding the setup of the latest recording, so a recording started from the tray or a shortcut can
/// repeat it.
const KEY_LAST_SETUP: &str = "last_recording_setup";

/// The longest preset name accepted (characters).
pub const MAX_NAME_CHARS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub id: i64,
    pub name: String,
    pub setup: RecordingSetup,
}

fn invalid(reason: &str) -> AppError {
    AppError::InvalidSetting {
        field: "presetName",
        reason: reason.to_string(),
    }
}

/// All presets, by name (ignoring case).
pub fn list(conn: &Connection) -> Result<Vec<Preset>, AppError> {
    let mut statement = conn
        .prepare("SELECT id, name, settings FROM recording_presets ORDER BY name COLLATE NOCASE")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut presets = Vec::new();
    for row in rows {
        let (id, name, json) = row?;
        match serde_json::from_str::<RecordingSetup>(&json) {
            Ok(setup) => presets.push(Preset { id, name, setup }),
            // The name is the user's own words, not meeting content, but is not needed to find the row.
            Err(_) => warn!(id, "a saved preset could not be read and is skipped"),
        }
    }
    Ok(presets)
}

/// Saves a setup under a name. A name that already exists (ignoring case) is replaced, so saving again updates it.
/// The title is not part of a preset: it belongs to one meeting.
pub fn save(conn: &Connection, name: &str, setup: &RecordingSetup) -> Result<Preset, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("the preset needs a name"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(invalid("the preset name is too long"));
    }
    let mut stored = setup.clone();
    stored.title = String::new();
    let json = serde_json::to_string(&stored)
        .expect("a recording setup is plain data and always serializes");
    conn.execute(
        "INSERT INTO recording_presets (name, settings) VALUES (?1, ?2)
         ON CONFLICT (name) DO UPDATE SET
             settings = excluded.settings,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        params![name, json],
    )?;
    let (id, name): (i64, String) = conn.query_row(
        "SELECT id, name FROM recording_presets WHERE name = ?1",
        [name],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(Preset {
        id,
        name,
        setup: stored,
    })
}

/// The setup the latest recording was started with, if there was one and it can still be read.
pub fn last_used(conn: &Connection) -> Result<Option<RecordingSetup>, AppError> {
    crate::storage::settings::read_key(conn, KEY_LAST_SETUP)
}

/// Remembers the setup of the recording that is starting. The title belongs to one meeting and is not kept.
pub fn save_last_used(conn: &Connection, setup: &RecordingSetup) -> Result<(), AppError> {
    let mut stored = setup.clone();
    stored.title = String::new();
    crate::storage::settings::write_key(conn, KEY_LAST_SETUP, &stored)
}

pub fn delete(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM recording_presets WHERE id = ?1", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::setup::Quality;
    use crate::storage::migrations::migrate;

    fn db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        migrate(&mut conn).unwrap();
        conn
    }

    fn setup(fps: u32) -> RecordingSetup {
        RecordingSetup {
            fps,
            quality: Quality::P720,
            ..RecordingSetup::default()
        }
    }

    #[test]
    fn a_saved_preset_comes_back_without_the_meeting_title() {
        let conn = db();
        let mut with_title = setup(10);
        with_title.title = "Tuesday standup".into();

        let saved = save(&conn, "Standup", &with_title).unwrap();

        assert_eq!(saved.setup.title, "", "a title belongs to one meeting");
        assert_eq!(list(&conn).unwrap(), vec![saved]);
    }

    #[test]
    fn saving_under_an_existing_name_replaces_it_ignoring_case() {
        let conn = db();
        let first = save(&conn, "Standup", &setup(5)).unwrap();

        let second = save(&conn, "  standup ", &setup(30)).unwrap();

        assert_eq!(second.id, first.id, "the same row is updated");
        let presets = list(&conn).unwrap();
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].setup.fps, 30);
        assert_eq!(presets[0].name, "Standup", "the first spelling is kept");
    }

    #[test]
    fn presets_are_listed_by_name_ignoring_case() {
        let conn = db();
        for name in ["beta", "Alpha", "gamma"] {
            save(&conn, name, &setup(15)).unwrap();
        }

        let names: Vec<_> = list(&conn).unwrap().into_iter().map(|p| p.name).collect();

        assert_eq!(names, ["Alpha", "beta", "gamma"]);
    }

    #[test]
    fn an_empty_or_huge_name_is_refused_and_nothing_is_saved() {
        let conn = db();

        assert!(save(&conn, "   ", &setup(15)).is_err());
        assert!(save(&conn, &"x".repeat(MAX_NAME_CHARS + 1), &setup(15)).is_err());

        assert!(list(&conn).unwrap().is_empty());
    }

    #[test]
    fn the_last_used_setup_is_remembered_without_its_title() {
        let conn = db();
        assert_eq!(last_used(&conn).unwrap(), None);
        let mut used = setup(10);
        used.title = "Board meeting".into();

        save_last_used(&conn, &used).unwrap();

        let remembered = last_used(&conn).unwrap().unwrap();
        assert_eq!(remembered.fps, 10);
        assert_eq!(remembered.title, "");
    }

    #[test]
    fn deleting_a_preset_removes_only_that_one() {
        let conn = db();
        let keep = save(&conn, "Keep", &setup(15)).unwrap();
        let drop = save(&conn, "Drop", &setup(15)).unwrap();

        delete(&conn, drop.id).unwrap();

        assert_eq!(list(&conn).unwrap(), vec![keep]);
    }

    #[test]
    fn a_preset_that_cannot_be_read_is_skipped_not_fatal() {
        let conn = db();
        let good = save(&conn, "Good", &setup(15)).unwrap();
        // Valid JSON for the database, but not a setup this build understands.
        conn.execute(
            "INSERT INTO recording_presets (name, settings) VALUES ('Broken', '{\"fps\": \"fast\"}')",
            [],
        )
        .unwrap();

        assert_eq!(list(&conn).unwrap(), vec![good]);
    }
}
