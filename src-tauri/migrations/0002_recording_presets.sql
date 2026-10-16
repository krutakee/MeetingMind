-- Named recording setups ("presets") for the Record screen (SPEC Section 5, Setup screen).
-- `settings` is a JSON document owned by the recording code; the database only checks that it is valid JSON.
-- Names are unique ignoring case, so "Standup" and "standup" cannot both exist.

CREATE TABLE recording_presets (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL COLLATE NOCASE UNIQUE CHECK (length(trim(name)) > 0),
    settings   TEXT    NOT NULL CHECK (json_valid(settings)),
    created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
