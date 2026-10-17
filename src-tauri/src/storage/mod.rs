//! SQLite storage. OS-agnostic: nothing in here may use platform APIs.

pub mod cloud;
pub mod meetings;
pub mod migrations;
pub mod presets;
pub mod settings;

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

use crate::error::AppError;
pub use settings::Settings;

/// The app database: one connection behind a mutex.
///
/// Fine for Phase 1 (a few small reads and writes). Phases 2-3 write transcript segments while recording, and
/// should move to a dedicated writer thread rather than sharing this lock with UI reads.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Opens (creating if needed) the database file and brings it to the latest schema.
    pub fn open(path: &Path) -> Result<Self, AppError> {
        Self::from_connection(Connection::open(path)?, true)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, AppError> {
        Self::from_connection(Connection::open_in_memory()?, false)
    }

    fn from_connection(mut conn: Connection, file_backed: bool) -> Result<Self, AppError> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        if file_backed {
            // WAL lets reads continue while a write is in progress. Not applicable to in-memory databases.
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "synchronous", "NORMAL")?;
        }
        migrations::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Runs `f` with exclusive use of the connection.
    pub fn with_conn<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let mut conn = self.conn.lock().map_err(|_| AppError::LockPoisoned)?;
        f(&mut conn)
    }

    pub fn schema_version(&self) -> Result<u32, AppError> {
        self.with_conn(|conn| migrations::current_version(conn))
    }

    pub fn load_settings(&self) -> Result<Settings, AppError> {
        self.with_conn(|conn| settings::load(conn))
    }

    pub fn save_settings(&self, new: &Settings) -> Result<Settings, AppError> {
        self.with_conn(|conn| settings::save(conn, new))
    }

    pub fn last_setup(&self) -> Result<Option<crate::recording::setup::RecordingSetup>, AppError> {
        self.with_conn(|conn| presets::last_used(conn))
    }

    pub fn save_last_setup(
        &self,
        setup: &crate::recording::setup::RecordingSetup,
    ) -> Result<(), AppError> {
        self.with_conn(|conn| presets::save_last_used(conn, setup))
    }
}

#[cfg(test)]
mod tests {
    use super::settings::Theme;
    use super::*;

    #[test]
    fn in_memory_database_is_migrated_and_enforces_foreign_keys() {
        let db = Database::open_in_memory().unwrap();

        assert_eq!(db.schema_version().unwrap(), migrations::latest_version());
        let foreign_keys: bool = db
            .with_conn(|conn| Ok(conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?))
            .unwrap();
        assert!(foreign_keys);
    }

    #[test]
    fn a_database_file_persists_settings_across_reopen_and_uses_wal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meetingmind.db");
        let wanted = Settings {
            storage_folder: None,
            theme: Theme::Dark,
            consent_reminder: false,
            hotkeys: settings::Hotkeys::default(),
        };

        {
            let db = Database::open(&path).unwrap();
            db.save_settings(&wanted).unwrap();
            let journal_mode: String = db
                .with_conn(|conn| Ok(conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?))
                .unwrap();
            assert_eq!(journal_mode.to_lowercase(), "wal");
        }

        let reopened = Database::open(&path).unwrap();
        assert_eq!(reopened.load_settings().unwrap(), wanted);
        assert_eq!(
            reopened.schema_version().unwrap(),
            migrations::latest_version(),
            "reopening must not re-run or duplicate migrations"
        );
    }
}
