-- Initial schema (SPEC.md Section 9).
--
-- Conventions:
--   * Timestamps are ISO-8601 UTC text, e.g. 2026-10-16T09:30:00.000Z.
--   * Media-timeline positions are integer milliseconds (paused time is already cut out, SPEC Section 5).
--   * JSON columns are guarded with json_valid().
--   * Foreign keys are enforced: the connection turns them on (PRAGMA foreign_keys cannot be set inside a migration).
--   * SQLite cannot alter a CHECK constraint in place, so constraints below are limited to values the spec fixes.
--     meetings.status is deliberately unconstrained here; its values are owned by the recording code (Phase 2).

CREATE TABLE meetings (
    id               INTEGER PRIMARY KEY,
    title            TEXT    NOT NULL,
    started_at       TEXT    NOT NULL,
    ended_at         TEXT,
    duration_ms      INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    mode             TEXT    NOT NULL CHECK (mode IN ('screen_audio', 'audio_only')),
    folder_path      TEXT    NOT NULL UNIQUE,
    status           TEXT    NOT NULL,
    capture_settings TEXT    CHECK (capture_settings IS NULL OR json_valid(capture_settings)),
    language_mode    TEXT    NOT NULL CHECK (language_mode IN ('single', 'auto', 'two_language')),
    models_used      TEXT    CHECK (models_used IS NULL OR json_valid(models_used)),
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_meetings_started_at ON meetings (started_at DESC);
CREATE INDEX idx_meetings_status     ON meetings (status);

-- original_text is what the model produced; edited_text is the user's edit and is kept apart (SPEC Section 7).
-- speaker_label is not a foreign key to speakers on purpose: recovered or imported segments must not be rejected
-- because a label row is missing.
CREATE TABLE segments (
    id            INTEGER PRIMARY KEY,
    meeting_id    INTEGER NOT NULL REFERENCES meetings (id) ON DELETE CASCADE,
    stream        TEXT    NOT NULL CHECK (stream IN ('mic', 'system')),
    speaker_label TEXT    NOT NULL,
    start_ms      INTEGER NOT NULL CHECK (start_ms >= 0),
    end_ms        INTEGER NOT NULL,
    original_text TEXT    NOT NULL,
    edited_text   TEXT,
    language      TEXT,
    is_final      INTEGER NOT NULL DEFAULT 0 CHECK (is_final IN (0, 1)),
    is_duplicate  INTEGER NOT NULL DEFAULT 0 CHECK (is_duplicate IN (0, 1)),
    pass          TEXT    NOT NULL DEFAULT 'live' CHECK (pass IN ('live', 'final')),
    CHECK (end_ms >= start_ms)
);

CREATE INDEX idx_segments_meeting_time ON segments (meeting_id, start_ms);

CREATE TABLE speakers (
    meeting_id   INTEGER NOT NULL REFERENCES meetings (id) ON DELETE CASCADE,
    label        TEXT    NOT NULL,
    display_name TEXT    NOT NULL,
    PRIMARY KEY (meeting_id, label)
) WITHOUT ROWID;

-- meeting_id is NULL for jobs that belong to no meeting (model downloads).
-- error must never contain transcript text (SPEC Section 10).
CREATE TABLE jobs (
    id          INTEGER PRIMARY KEY,
    meeting_id  INTEGER REFERENCES meetings (id) ON DELETE CASCADE,
    type        TEXT    NOT NULL CHECK (type IN ('final_pass', 'export', 'recovery', 'model_download')),
    status      TEXT    NOT NULL DEFAULT 'queued'
                        CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'cancelled')),
    progress    REAL    NOT NULL DEFAULT 0 CHECK (progress >= 0 AND progress <= 1),
    payload     TEXT    CHECK (payload IS NULL OR json_valid(payload)),
    error       TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    started_at  TEXT,
    finished_at TEXT
);

CREATE INDEX idx_jobs_status  ON jobs (status);
CREATE INDEX idx_jobs_meeting ON jobs (meeting_id);

CREATE TABLE models (
    name         TEXT    PRIMARY KEY,
    file_path    TEXT    NOT NULL,
    size_bytes   INTEGER NOT NULL CHECK (size_bytes >= 0),
    sha256       TEXT    NOT NULL CHECK (length(sha256) = 64),
    installed_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) WITHOUT ROWID;

CREATE TABLE settings (
    key        TEXT NOT NULL PRIMARY KEY,
    value      TEXT NOT NULL CHECK (json_valid(value)),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) WITHOUT ROWID;

-- Full-text search over segments. The index holds the text the user sees: the edit if there is one, otherwise the
-- original. It is kept in sync by the triggers below, so it never needs a manual rebuild.
-- Note for Phase 4: the unicode61 tokenizer does not split Chinese/Japanese into words; revisit (e.g. trigram)
-- when search is built, as a new migration.
CREATE VIRTUAL TABLE segments_fts USING fts5 (
    text,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER segments_fts_insert AFTER INSERT ON segments BEGIN
    INSERT INTO segments_fts (rowid, text) VALUES (new.id, COALESCE(new.edited_text, new.original_text));
END;

CREATE TRIGGER segments_fts_update AFTER UPDATE OF original_text, edited_text ON segments BEGIN
    DELETE FROM segments_fts WHERE rowid = old.id;
    INSERT INTO segments_fts (rowid, text) VALUES (new.id, COALESCE(new.edited_text, new.original_text));
END;

CREATE TRIGGER segments_fts_delete AFTER DELETE ON segments BEGIN
    DELETE FROM segments_fts WHERE rowid = old.id;
END;
