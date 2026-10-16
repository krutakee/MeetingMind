//! Meeting rows: created when a recording starts, updated as it goes, and read back to find unfinished recordings.
//!
//! Statuses used by recording: `recording`, `finalizing`, `complete`, `failed`, `cancelled`
//! (see `recording::state::RecordingState::db_status`).

use rusqlite::{Connection, params};

use crate::error::AppError;

pub struct NewMeeting<'a> {
    pub title: &'a str,
    /// RFC 3339, UTC.
    pub started_at: &'a str,
    /// `screen_audio` or `audio_only`.
    pub mode: &'a str,
    pub folder_path: &'a str,
    /// JSON describing the capture setup, if any.
    pub capture_settings: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingRow {
    pub id: i64,
    pub title: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub mode: String,
    pub folder_path: String,
    pub status: String,
}

const COLUMNS: &str = "id, title, started_at, ended_at, duration_ms, mode, folder_path, status";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MeetingRow> {
    Ok(MeetingRow {
        id: r.get(0)?,
        title: r.get(1)?,
        started_at: r.get(2)?,
        ended_at: r.get(3)?,
        duration_ms: r.get(4)?,
        mode: r.get(5)?,
        folder_path: r.get(6)?,
        status: r.get(7)?,
    })
}

/// Inserts a meeting with status `recording` and returns its id. Phase 2 has no language setting yet, so
/// `language_mode` takes its only sensible value, `single`.
pub fn create(conn: &Connection, meeting: &NewMeeting<'_>) -> Result<i64, AppError> {
    conn.execute(
        "INSERT INTO meetings (title, started_at, mode, folder_path, status, capture_settings, language_mode)
         VALUES (?1, ?2, ?3, ?4, 'recording', ?5, 'single')",
        params![meeting.title, meeting.started_at, meeting.mode, meeting.folder_path, meeting.capture_settings],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn set_status(conn: &Connection, id: i64, status: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE meetings SET status = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
        params![id, status],
    )?;
    Ok(())
}

/// Records how a recording ended.
pub fn finish(
    conn: &Connection,
    id: i64,
    status: &str,
    ended_at: &str,
    duration_ms: i64,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE meetings
            SET status = ?2, ended_at = ?3, duration_ms = ?4, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE id = ?1",
        params![id, status, ended_at, duration_ms],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<MeetingRow>, AppError> {
    let sql = format!("SELECT {COLUMNS} FROM meetings WHERE id = ?1");
    match conn.query_row(&sql, [id], row) {
        Ok(found) => Ok(Some(found)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Meetings that were being recorded or finalized when the app last stopped: crash recovery works from this.
/// Oldest first.
pub fn unfinished(conn: &Connection) -> Result<Vec<MeetingRow>, AppError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM meetings WHERE status IN ('recording', 'finalizing') ORDER BY started_at, id"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([], row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// The most recent meetings that finished, newest first.
pub fn recent_complete(conn: &Connection, limit: u32) -> Result<Vec<MeetingRow>, AppError> {
    let sql = format!(
        "SELECT {COLUMNS} FROM meetings WHERE status = 'complete' ORDER BY started_at DESC, id DESC LIMIT ?1"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([limit], row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Removes the meeting row. Segments, speakers and jobs go with it (foreign keys cascade). The files are not touched.
pub fn delete(conn: &Connection, id: i64) -> Result<(), AppError> {
    conn.execute("DELETE FROM meetings WHERE id = ?1", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations::migrate;

    fn db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        migrate(&mut conn).unwrap();
        conn
    }

    fn new_meeting<'a>(title: &'a str, folder: &'a str, started_at: &'a str) -> NewMeeting<'a> {
        NewMeeting {
            title,
            started_at,
            mode: "screen_audio",
            folder_path: folder,
            capture_settings: None,
        }
    }

    #[test]
    fn a_new_meeting_starts_as_recording() {
        let conn = db();

        let id = create(
            &conn,
            &new_meeting("Standup", "C:/m/1", "2026-10-16T09:00:00.000Z"),
        )
        .unwrap();

        let found = get(&conn, id).unwrap().unwrap();
        assert_eq!(found.title, "Standup");
        assert_eq!(found.status, "recording");
        assert_eq!(found.ended_at, None);
        assert_eq!(found.duration_ms, None);
    }

    #[test]
    fn finishing_records_the_end_and_the_media_duration() {
        let conn = db();
        let id = create(
            &conn,
            &new_meeting("A", "C:/m/1", "2026-10-16T09:00:00.000Z"),
        )
        .unwrap();

        finish(&conn, id, "complete", "2026-10-16T09:30:00.000Z", 1_740_000).unwrap();

        let found = get(&conn, id).unwrap().unwrap();
        assert_eq!(found.status, "complete");
        assert_eq!(found.ended_at.as_deref(), Some("2026-10-16T09:30:00.000Z"));
        assert_eq!(found.duration_ms, Some(1_740_000));
    }

    #[test]
    fn only_unfinished_meetings_are_listed_for_recovery_oldest_first() {
        let conn = db();
        let late = create(
            &conn,
            &new_meeting("late", "C:/m/late", "2026-10-16T11:00:00.000Z"),
        )
        .unwrap();
        let early = create(
            &conn,
            &new_meeting("early", "C:/m/early", "2026-10-16T08:00:00.000Z"),
        )
        .unwrap();
        let finalizing = create(
            &conn,
            &new_meeting("fin", "C:/m/fin", "2026-10-16T09:00:00.000Z"),
        )
        .unwrap();
        set_status(&conn, finalizing, "finalizing").unwrap();
        let done = create(
            &conn,
            &new_meeting("done", "C:/m/done", "2026-10-16T07:00:00.000Z"),
        )
        .unwrap();
        finish(&conn, done, "complete", "2026-10-16T07:10:00.000Z", 1).unwrap();
        let failed = create(
            &conn,
            &new_meeting("failed", "C:/m/failed", "2026-10-16T06:00:00.000Z"),
        )
        .unwrap();
        set_status(&conn, failed, "failed").unwrap();
        let cancelled = create(
            &conn,
            &new_meeting("cancelled", "C:/m/c", "2026-10-16T05:00:00.000Z"),
        )
        .unwrap();
        set_status(&conn, cancelled, "cancelled").unwrap();

        let ids: Vec<i64> = unfinished(&conn).unwrap().iter().map(|m| m.id).collect();

        assert_eq!(ids, vec![early, finalizing, late]);
    }

    #[test]
    fn recent_complete_meetings_come_newest_first_and_respect_the_limit() {
        let conn = db();
        for (i, started) in ["2026-10-14", "2026-10-16", "2026-10-15"]
            .iter()
            .enumerate()
        {
            let id = create(
                &conn,
                &new_meeting(
                    "m",
                    &format!("C:/m/{i}"),
                    &format!("{started}T09:00:00.000Z"),
                ),
            )
            .unwrap();
            finish(
                &conn,
                id,
                "complete",
                &format!("{started}T09:10:00.000Z"),
                1000,
            )
            .unwrap();
        }
        let _unfinished = create(
            &conn,
            &new_meeting("running", "C:/m/run", "2026-10-17T09:00:00.000Z"),
        )
        .unwrap();

        let recent = recent_complete(&conn, 2).unwrap();

        let days: Vec<&str> = recent.iter().map(|m| &m.started_at[..10]).collect();
        assert_eq!(days, vec!["2026-10-16", "2026-10-15"]);
    }

    #[test]
    fn two_meetings_cannot_share_a_folder() {
        let conn = db();
        create(
            &conn,
            &new_meeting("a", "C:/m/same", "2026-10-16T09:00:00.000Z"),
        )
        .unwrap();

        let second = create(
            &conn,
            &new_meeting("b", "C:/m/same", "2026-10-16T10:00:00.000Z"),
        );

        assert!(second.is_err());
    }

    #[test]
    fn deleting_a_meeting_removes_the_row() {
        let conn = db();
        let id = create(
            &conn,
            &new_meeting("a", "C:/m/1", "2026-10-16T09:00:00.000Z"),
        )
        .unwrap();

        delete(&conn, id).unwrap();

        assert_eq!(get(&conn, id).unwrap(), None);
    }

    #[test]
    fn an_unknown_id_is_none_not_an_error() {
        assert_eq!(get(&db(), 12345).unwrap(), None);
    }
}
