use serde::{Serialize, Serializer, ser::SerializeStruct};

/// Every error the backend can hand to the UI.
///
/// Messages are shown to the user and written to logs, so they must never contain transcript text.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration {version} ({name}) failed: {source}")]
    MigrationFailed {
        version: u32,
        name: String,
        #[source]
        source: rusqlite::Error,
    },

    #[error(
        "the database was created by a newer version of MeetingMind (schema v{found}; this build supports up to v{supported})"
    )]
    DatabaseTooNew { found: u32, supported: u32 },

    #[error(
        "the database migration history does not match this build at version {version}: database has '{found}', this build expects '{expected}'"
    )]
    MigrationMismatch {
        version: u32,
        expected: String,
        found: String,
    },

    #[error("invalid migration history: {0}")]
    InvalidMigrations(String),

    #[error("invalid setting '{field}': {reason}")]
    InvalidSetting { field: &'static str, reason: String },

    #[error("file system error: {0}")]
    Io(#[from] std::io::Error),

    #[error("could not resolve app folders: {0}")]
    Paths(String),

    #[error("logging could not be started: {0}")]
    Logging(String),

    #[error("the app did not start correctly: {0}")]
    Unavailable(String),

    #[error("internal error: a lock was poisoned by an earlier failure")]
    LockPoisoned,

    /// The request was understood and refused (a folder outside the storage folder, a recording that is not waiting
    /// for recovery).
    #[error("{0}")]
    Refused(String),
}

impl AppError {
    fn kind(&self) -> &'static str {
        match self {
            AppError::Sqlite(_) => "database",
            AppError::MigrationFailed { .. } => "migrationFailed",
            AppError::DatabaseTooNew { .. } => "databaseTooNew",
            AppError::MigrationMismatch { .. } => "migrationMismatch",
            AppError::InvalidMigrations(_) => "invalidMigrations",
            AppError::InvalidSetting { .. } => "invalidSetting",
            AppError::Io(_) => "io",
            AppError::Paths(_) => "paths",
            AppError::Logging(_) => "logging",
            AppError::Unavailable(_) => "unavailable",
            AppError::LockPoisoned => "lockPoisoned",
            AppError::Refused(_) => "refused",
        }
    }
}

/// Serialized for the frontend as `{ "kind": "...", "message": "..." }`.
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("AppError", 2)?;
        state.serialize_field("kind", self.kind())?;
        state.serialize_field("message", &self.to_string())?;
        state.end()
    }
}
