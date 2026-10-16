//! Versioned, forward-only SQLite migrations.
//!
//! Each migration runs in its own transaction together with its `schema_migrations` row, so a failing migration
//! leaves the database exactly as it was before that migration. Applied migrations are never edited: a change to
//! the schema is always a new numbered file in `src-tauri/migrations/`.

use rusqlite::{Connection, TransactionBehavior};
use tracing::info;

use crate::error::AppError;

pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// All migrations, in order. Versions must be 1, 2, 3, ... with no gaps.
pub static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial_schema",
        sql: include_str!("../../migrations/0001_initial_schema.sql"),
    },
    Migration {
        version: 2,
        name: "recording_presets",
        sql: include_str!("../../migrations/0002_recording_presets.sql"),
    },
];

#[cfg(test)]
pub fn latest_version() -> u32 {
    MIGRATIONS.len() as u32
}

/// Brings the database up to the latest schema. Returns the versions that were applied by this call
/// (empty when the database was already current).
pub fn migrate(conn: &mut Connection) -> Result<Vec<u32>, AppError> {
    migrate_with(conn, MIGRATIONS)
}

/// The schema version of the database, or 0 if no migration has ever been applied.
pub fn current_version(conn: &Connection) -> Result<u32, AppError> {
    let table_exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |row| row.get(0),
    )?;
    if !table_exists {
        return Ok(0);
    }
    let version: Option<u32> =
        conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    Ok(version.unwrap_or(0))
}

pub(crate) fn migrate_with(
    conn: &mut Connection,
    migrations: &[Migration],
) -> Result<Vec<u32>, AppError> {
    check_migration_list(migrations)?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version    INTEGER PRIMARY KEY,
             name       TEXT NOT NULL,
             applied_at TEXT NOT NULL
         ) WITHOUT ROWID;",
    )?;

    let applied = applied_history(conn)?;
    check_history(&applied, migrations)?;

    let mut newly_applied = Vec::new();
    for migration in migrations.iter().skip(applied.len()) {
        if apply(conn, migration)? {
            info!(
                version = migration.version,
                name = migration.name,
                "database migration applied"
            );
            newly_applied.push(migration.version);
        }
    }
    Ok(newly_applied)
}

fn check_migration_list(migrations: &[Migration]) -> Result<(), AppError> {
    for (index, migration) in migrations.iter().enumerate() {
        let expected = index as u32 + 1;
        if migration.version != expected {
            return Err(AppError::InvalidMigrations(format!(
                "migration '{}' has version {} but is at position {}; versions must be 1, 2, 3, ... in order",
                migration.name, migration.version, expected
            )));
        }
    }
    Ok(())
}

fn applied_history(conn: &Connection) -> Result<Vec<(u32, String)>, AppError> {
    let mut statement =
        conn.prepare("SELECT version, name FROM schema_migrations ORDER BY version")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn check_history(applied: &[(u32, String)], migrations: &[Migration]) -> Result<(), AppError> {
    for (index, (version, _)) in applied.iter().enumerate() {
        let expected = index as u32 + 1;
        if *version != expected {
            return Err(AppError::InvalidMigrations(format!(
                "applied versions are not contiguous: expected {expected}, found {version}"
            )));
        }
    }
    if applied.len() > migrations.len() {
        return Err(AppError::DatabaseTooNew {
            found: applied.len() as u32,
            supported: migrations.len() as u32,
        });
    }
    for ((version, name), migration) in applied.iter().zip(migrations) {
        if name != migration.name {
            return Err(AppError::MigrationMismatch {
                version: *version,
                expected: migration.name.to_string(),
                found: name.clone(),
            });
        }
    }
    Ok(())
}

/// Applies one migration. Returns false if another process applied it first.
fn apply(conn: &mut Connection, migration: &Migration) -> Result<bool, AppError> {
    let failed = |source| AppError::MigrationFailed {
        version: migration.version,
        name: migration.name.to_string(),
        source,
    };

    // IMMEDIATE takes the write lock up front, so two app instances starting together cannot both apply it.
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;

    let already_applied: bool = tx
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM schema_migrations WHERE version = ?1)",
            [migration.version],
            |row| row.get(0),
        )
        .map_err(failed)?;
    if already_applied {
        return Ok(false);
    }

    tx.execute_batch(migration.sql).map_err(failed)?;
    tx.execute(
        "INSERT INTO schema_migrations (version, name, applied_at)
         VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        rusqlite::params![migration.version, migration.name],
    )
    .map_err(failed)?;
    tx.commit().map_err(failed)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn
    }

    fn applied_versions(conn: &Connection) -> Vec<(u32, String)> {
        applied_history(conn).unwrap()
    }

    /// Schema as SQLite reports it, for comparing two databases. Skips SQLite's internal tables.
    fn schema_snapshot(conn: &Connection) -> Vec<(String, String)> {
        let mut statement = conn
            .prepare(
                "SELECT name, sql FROM sqlite_master
                 WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn insert_meeting(conn: &Connection, folder: &str) -> i64 {
        conn.execute(
            "INSERT INTO meetings (title, started_at, mode, folder_path, status, language_mode)
             VALUES ('Weekly sync', '2026-10-16T09:00:00.000Z', 'audio_only', ?1, 'recording', 'single')",
            [folder],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_segment(conn: &Connection, meeting_id: i64, text: &str) -> i64 {
        conn.execute(
            "INSERT INTO segments (meeting_id, stream, speaker_label, start_ms, end_ms, original_text)
             VALUES (?1, 'mic', 'You', 0, 1500, ?2)",
            rusqlite::params![meeting_id, text],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn fts_matches(conn: &Connection, query: &str) -> Vec<i64> {
        let mut statement = conn
            .prepare("SELECT rowid FROM segments_fts WHERE segments_fts MATCH ?1 ORDER BY rowid")
            .unwrap();
        statement
            .query_map([query], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    // --- the real migration list ---

    #[test]
    fn fresh_database_reaches_latest_version() {
        let mut conn = connection();
        assert_eq!(current_version(&conn).unwrap(), 0);

        let applied = migrate(&mut conn).unwrap();

        assert_eq!(applied, (1..=latest_version()).collect::<Vec<_>>());
        assert_eq!(current_version(&conn).unwrap(), latest_version());
    }

    #[test]
    fn real_migration_list_is_well_formed() {
        check_migration_list(MIGRATIONS).unwrap();
        assert!(latest_version() >= 1);
    }

    #[test]
    fn every_expected_table_exists_after_migrating() {
        let mut conn = connection();
        migrate(&mut conn).unwrap();

        for table in [
            "meetings",
            "segments",
            "speakers",
            "jobs",
            "models",
            "settings",
            "recording_presets",
            "segments_fts",
            "schema_migrations",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = ?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "table {table} is missing");
        }
    }

    #[test]
    fn rerunning_migrations_is_a_no_op() {
        let mut conn = connection();
        migrate(&mut conn).unwrap();
        let history_before: Vec<(u32, String, String)> = conn
            .prepare("SELECT version, name, applied_at FROM schema_migrations ORDER BY version")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let schema_before = schema_snapshot(&conn);

        let applied_again = migrate(&mut conn).unwrap();

        assert!(applied_again.is_empty());
        let history_after: Vec<(u32, String, String)> = conn
            .prepare("SELECT version, name, applied_at FROM schema_migrations ORDER BY version")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(history_before, history_after);
        assert_eq!(schema_before, schema_snapshot(&conn));
    }

    #[test]
    fn upgrading_from_every_earlier_version_matches_a_fresh_database() {
        let mut fresh = connection();
        migrate(&mut fresh).unwrap();
        let expected_schema = schema_snapshot(&fresh);

        for start in 0..latest_version() {
            let mut conn = connection();
            // Bring the database to `start` using only the first `start` real migrations.
            migrate_with(&mut conn, &MIGRATIONS[..start as usize]).unwrap();
            assert_eq!(current_version(&conn).unwrap(), start);

            let applied = migrate(&mut conn).unwrap();

            assert_eq!(
                applied,
                ((start + 1)..=latest_version()).collect::<Vec<_>>(),
                "upgrade from v{start}"
            );
            assert_eq!(
                schema_snapshot(&conn),
                expected_schema,
                "schema after upgrading from v{start}"
            );
        }
    }

    #[test]
    fn fts5_is_compiled_in_and_the_index_follows_the_segments() {
        let mut conn = connection();
        migrate(&mut conn).unwrap();

        let fts5: bool = conn
            .query_row("SELECT sqlite_compileoption_used('ENABLE_FTS5')", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(fts5, "SQLite was built without FTS5");

        let meeting = insert_meeting(&conn, "C:/m/1");
        let first = insert_segment(&conn, meeting, "Budget review for the Zürich office");
        let second = insert_segment(&conn, meeting, "Next steps and owners");

        assert_eq!(fts_matches(&conn, "budget"), vec![first]);
        assert_eq!(
            fts_matches(&conn, "zurich"),
            vec![first],
            "diacritics are folded"
        );

        // A user edit replaces what is searchable; the original stays in the table.
        conn.execute(
            "UPDATE segments SET edited_text = 'Forecast review' WHERE id = ?1",
            [first],
        )
        .unwrap();
        assert!(fts_matches(&conn, "budget").is_empty());
        assert_eq!(fts_matches(&conn, "forecast"), vec![first]);
        let original: String = conn
            .query_row(
                "SELECT original_text FROM segments WHERE id = ?1",
                [first],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(original, "Budget review for the Zürich office");

        // Clearing the edit makes the original searchable again.
        conn.execute(
            "UPDATE segments SET edited_text = NULL WHERE id = ?1",
            [first],
        )
        .unwrap();
        assert_eq!(fts_matches(&conn, "budget"), vec![first]);

        conn.execute("DELETE FROM segments WHERE id = ?1", [second])
            .unwrap();
        assert!(fts_matches(&conn, "owners").is_empty());
    }

    #[test]
    fn deleting_a_meeting_removes_its_rows_and_search_entries() {
        let mut conn = connection();
        migrate(&mut conn).unwrap();
        let meeting = insert_meeting(&conn, "C:/m/1");
        insert_segment(&conn, meeting, "searchable words");
        conn.execute(
            "INSERT INTO speakers (meeting_id, label, display_name) VALUES (?1, 'You', 'Alex')",
            [meeting],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO jobs (meeting_id, type) VALUES (?1, 'final_pass')",
            [meeting],
        )
        .unwrap();

        conn.execute("DELETE FROM meetings WHERE id = ?1", [meeting])
            .unwrap();

        for table in ["segments", "speakers", "jobs"] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table} rows survived the meeting delete");
        }
        assert!(
            fts_matches(&conn, "searchable").is_empty(),
            "cascaded delete must also clear the search index"
        );
    }

    #[test]
    fn foreign_keys_and_checks_reject_bad_rows() {
        let mut conn = connection();
        migrate(&mut conn).unwrap();
        let meeting = insert_meeting(&conn, "C:/m/1");

        let orphan = conn.execute(
            "INSERT INTO segments (meeting_id, stream, speaker_label, start_ms, end_ms, original_text)
             VALUES (9999, 'mic', 'You', 0, 1, 'x')",
            [],
        );
        assert!(
            orphan.is_err(),
            "segment for a missing meeting was accepted"
        );

        let backwards = conn.execute(
            "INSERT INTO segments (meeting_id, stream, speaker_label, start_ms, end_ms, original_text)
             VALUES (?1, 'mic', 'You', 2000, 1000, 'x')",
            [meeting],
        );
        assert!(
            backwards.is_err(),
            "segment ending before it starts was accepted"
        );

        let bad_stream = conn.execute(
            "INSERT INTO segments (meeting_id, stream, speaker_label, start_ms, end_ms, original_text)
             VALUES (?1, 'speaker', 'You', 0, 1, 'x')",
            [meeting],
        );
        assert!(bad_stream.is_err(), "unknown stream was accepted");

        let duplicate_folder = conn.execute(
            "INSERT INTO meetings (title, started_at, mode, folder_path, status, language_mode)
             VALUES ('Again', '2026-10-16T10:00:00.000Z', 'audio_only', 'C:/m/1', 'recording', 'single')",
            [],
        );
        assert!(duplicate_folder.is_err(), "two meetings share one folder");
    }

    #[test]
    fn preset_names_are_unique_ignoring_case_and_settings_must_be_json() {
        let mut conn = connection();
        migrate(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO recording_presets (name, settings) VALUES ('Standup', '{\"fps\":15}')",
            [],
        )
        .unwrap();

        let same_name_other_case = conn.execute(
            "INSERT INTO recording_presets (name, settings) VALUES ('STANDUP', '{}')",
            [],
        );
        let not_json = conn.execute(
            "INSERT INTO recording_presets (name, settings) VALUES ('Other', 'not json')",
            [],
        );
        let blank_name = conn.execute(
            "INSERT INTO recording_presets (name, settings) VALUES ('   ', '{}')",
            [],
        );

        assert!(
            same_name_other_case.is_err(),
            "names must be unique ignoring case"
        );
        assert!(not_json.is_err(), "settings must be valid JSON");
        assert!(blank_name.is_err(), "a blank name is not a name");
    }

    // --- runner behaviour, with small throwaway migration lists ---
    // The migrations below are test fixtures, not part of the product schema.

    const FIXTURE_STEPS: [Migration; 3] = [
        Migration {
            version: 1,
            name: "create_a",
            sql: "CREATE TABLE a (id INTEGER PRIMARY KEY);",
        },
        Migration {
            version: 2,
            name: "create_b",
            sql: "CREATE TABLE b (id INTEGER PRIMARY KEY);",
        },
        Migration {
            version: 3,
            name: "create_c",
            sql: "CREATE TABLE c (id INTEGER PRIMARY KEY);",
        },
    ];

    #[test]
    fn migrations_apply_one_at_a_time_in_order() {
        let mut conn = connection();

        assert_eq!(
            migrate_with(&mut conn, &FIXTURE_STEPS[..1]).unwrap(),
            vec![1]
        );
        assert_eq!(current_version(&conn).unwrap(), 1);

        assert_eq!(
            migrate_with(&mut conn, &FIXTURE_STEPS).unwrap(),
            vec![2, 3],
            "only the missing steps run, in order"
        );
        assert_eq!(
            applied_versions(&conn),
            vec![
                (1, "create_a".to_string()),
                (2, "create_b".to_string()),
                (3, "create_c".to_string())
            ]
        );
    }

    #[test]
    fn a_failing_migration_rolls_back_completely_and_keeps_earlier_ones() {
        let mut conn = connection();
        let steps = [
            Migration {
                version: 1,
                name: "create_a",
                sql: "CREATE TABLE a (id INTEGER PRIMARY KEY);",
            },
            Migration {
                version: 2,
                name: "half_broken",
                // The first statement succeeds, the second fails: neither may survive.
                sql: "CREATE TABLE b (id INTEGER PRIMARY KEY); INSERT INTO missing_table VALUES (1);",
            },
        ];

        let error = migrate_with(&mut conn, &steps).unwrap_err();

        assert!(
            matches!(error, AppError::MigrationFailed { version: 2, .. }),
            "got {error:?}"
        );
        assert_eq!(
            current_version(&conn).unwrap(),
            1,
            "migration 1 stays applied"
        );
        let b_exists: bool = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'b')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!b_exists, "table from the failed migration was left behind");
    }

    #[test]
    fn a_database_from_a_newer_build_is_refused() {
        let mut conn = connection();
        migrate_with(&mut conn, &FIXTURE_STEPS).unwrap();

        let error = migrate_with(&mut conn, &FIXTURE_STEPS[..2]).unwrap_err();

        assert!(
            matches!(
                error,
                AppError::DatabaseTooNew {
                    found: 3,
                    supported: 2
                }
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn a_renamed_migration_is_detected() {
        let mut conn = connection();
        migrate_with(&mut conn, &FIXTURE_STEPS).unwrap();
        let renamed = [
            Migration {
                version: 1,
                name: "create_a",
                sql: "",
            },
            Migration {
                version: 2,
                name: "something_else",
                sql: "",
            },
            Migration {
                version: 3,
                name: "create_c",
                sql: "",
            },
        ];

        let error = migrate_with(&mut conn, &renamed).unwrap_err();

        assert!(
            matches!(error, AppError::MigrationMismatch { version: 2, .. }),
            "got {error:?}"
        );
    }

    #[test]
    fn a_migration_list_with_gaps_is_rejected() {
        let mut conn = connection();
        let gapped = [
            Migration {
                version: 1,
                name: "one",
                sql: "",
            },
            Migration {
                version: 3,
                name: "three",
                sql: "",
            },
        ];

        let error = migrate_with(&mut conn, &gapped).unwrap_err();

        assert!(
            matches!(error, AppError::InvalidMigrations(_)),
            "got {error:?}"
        );
    }
}
