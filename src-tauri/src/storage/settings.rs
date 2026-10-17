//! App preferences, persisted in the `settings` table as one JSON value per key.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tracing::warn;

use crate::error::AppError;

const KEY_STORAGE_FOLDER: &str = "storage_folder";
const KEY_THEME: &str = "theme";
const KEY_CONSENT_REMINDER: &str = "consent_reminder";
const KEY_HOTKEYS: &str = "hotkeys";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Global keyboard shortcuts, as accelerator text such as `Ctrl+Shift+S`. Whether a combination can really be
/// registered (it may belong to another program) is only known when registering, which reports a clear message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hotkeys {
    /// Starts a recording, or stops the one that is running.
    pub start_stop: String,
    /// Pauses a recording, or resumes a paused one.
    pub pause_resume: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            start_stop: "Ctrl+Shift+S".to_string(),
            pause_resume: "Ctrl+Shift+P".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Where meeting folders are saved. `None` means "use the default folder".
    pub storage_folder: Option<String>,
    pub theme: Theme,
    /// Show the participant-consent reminder before a recording starts (SPEC Section 5).
    pub consent_reminder: bool,
    pub hotkeys: Hotkeys,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            storage_folder: None,
            theme: Theme::default(),
            consent_reminder: true,
            hotkeys: Hotkeys::default(),
        }
    }
}

/// Reads all settings. A key that was never saved, or whose stored value cannot be read, takes its default.
pub fn load(conn: &Connection) -> Result<Settings, AppError> {
    let defaults = Settings::default();
    Ok(Settings {
        storage_folder: read_key::<Option<String>>(conn, KEY_STORAGE_FOLDER)?
            .unwrap_or(defaults.storage_folder),
        theme: read_key(conn, KEY_THEME)?.unwrap_or(defaults.theme),
        consent_reminder: read_key(conn, KEY_CONSENT_REMINDER)?
            .unwrap_or(defaults.consent_reminder),
        hotkeys: read_key(conn, KEY_HOTKEYS)?.unwrap_or(defaults.hotkeys),
    })
}

/// Validates and saves all settings in one transaction: either every value is stored or none is.
/// Returns the values as stored (for example, with surrounding whitespace trimmed from the folder).
pub fn save(conn: &mut Connection, settings: &Settings) -> Result<Settings, AppError> {
    let valid = validate(settings)?;

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    write_key(&tx, KEY_STORAGE_FOLDER, &valid.storage_folder)?;
    write_key(&tx, KEY_THEME, &valid.theme)?;
    write_key(&tx, KEY_CONSENT_REMINDER, &valid.consent_reminder)?;
    write_key(&tx, KEY_HOTKEYS, &valid.hotkeys)?;
    tx.commit()?;

    Ok(valid)
}

fn validate(settings: &Settings) -> Result<Settings, AppError> {
    let mut valid = settings.clone();
    if let Some(folder) = &settings.storage_folder {
        let folder = folder.trim();
        let reject = |reason: &str| AppError::InvalidSetting {
            field: "storageFolder",
            reason: reason.to_string(),
        };
        if folder.is_empty() {
            return Err(reject("the folder path is empty"));
        }
        if folder.contains('\0') {
            return Err(reject("the folder path contains an invalid character"));
        }
        if !Path::new(folder).is_absolute() {
            return Err(reject("the folder path must be absolute"));
        }
        valid.storage_folder = Some(folder.to_string());
    }
    let keys = [
        ("hotkeys.startStop", &settings.hotkeys.start_stop),
        ("hotkeys.pauseResume", &settings.hotkeys.pause_resume),
    ];
    for (field, keys) in keys {
        let reject = |reason: &str| AppError::InvalidSetting {
            field,
            reason: reason.to_string(),
        };
        if keys.trim().is_empty() {
            return Err(reject("the shortcut is empty"));
        }
        if keys.chars().any(char::is_control) {
            return Err(reject("the shortcut contains an invalid character"));
        }
    }
    valid.hotkeys.start_stop = settings.hotkeys.start_stop.trim().to_string();
    valid.hotkeys.pause_resume = settings.hotkeys.pause_resume.trim().to_string();
    if valid
        .hotkeys
        .start_stop
        .eq_ignore_ascii_case(&valid.hotkeys.pause_resume)
    {
        return Err(AppError::InvalidSetting {
            field: "hotkeys.pauseResume",
            reason: "start/stop and pause/resume cannot use the same shortcut".to_string(),
        });
    }
    Ok(valid)
}

pub(crate) fn read_key<T: DeserializeOwned>(
    conn: &Connection,
    key: &str,
) -> Result<Option<T>, AppError> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    match serde_json::from_str(&raw) {
        Ok(value) => Ok(Some(value)),
        Err(_) => {
            // The value itself is not logged: it may be a file path.
            warn!(key, "stored setting could not be read; using its default");
            Ok(None)
        }
    }
}

pub(crate) fn write_key<T: Serialize>(
    conn: &Connection,
    key: &str,
    value: &T,
) -> Result<(), AppError> {
    let json =
        serde_json::to_string(value).expect("settings values are plain data and always serialize");
    conn.execute(
        "INSERT INTO settings (key, value, updated_at)
         VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![key, json],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations::migrate;

    fn migrated() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        migrate(&mut conn).unwrap();
        conn
    }

    fn some_absolute_folder() -> String {
        std::env::temp_dir()
            .join("MeetingMind-test")
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn a_fresh_database_returns_the_defaults() {
        let conn = migrated();

        let settings = load(&conn).unwrap();

        assert_eq!(settings, Settings::default());
        assert_eq!(settings.storage_folder, None);
        assert_eq!(settings.theme, Theme::System);
        assert!(settings.consent_reminder, "consent reminder defaults to on");
    }

    #[test]
    fn saved_settings_come_back_unchanged() {
        let mut conn = migrated();
        let wanted = Settings {
            storage_folder: Some(some_absolute_folder()),
            theme: Theme::Dark,
            consent_reminder: false,
            hotkeys: Hotkeys {
                start_stop: "Ctrl+Alt+R".into(),
                pause_resume: "Ctrl+Alt+P".into(),
            },
        };

        let stored = save(&mut conn, &wanted).unwrap();

        assert_eq!(stored, wanted);
        assert_eq!(load(&conn).unwrap(), wanted);
    }

    #[test]
    fn saving_again_replaces_values_and_a_folder_can_be_cleared() {
        let mut conn = migrated();
        save(
            &mut conn,
            &Settings {
                storage_folder: Some(some_absolute_folder()),
                theme: Theme::Light,
                ..Settings::default()
            },
        )
        .unwrap();

        save(
            &mut conn,
            &Settings {
                storage_folder: None,
                theme: Theme::Dark,
                ..Settings::default()
            },
        )
        .unwrap();

        let loaded = load(&conn).unwrap();
        assert_eq!(loaded.storage_folder, None);
        assert_eq!(loaded.theme, Theme::Dark);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 4, "one row per setting, none duplicated");
    }

    #[test]
    fn the_folder_is_trimmed() {
        let mut conn = migrated();
        let folder = some_absolute_folder();

        let stored = save(
            &mut conn,
            &Settings {
                storage_folder: Some(format!("  {folder}  ")),
                ..Settings::default()
            },
        )
        .unwrap();

        assert_eq!(stored.storage_folder, Some(folder));
    }

    #[test]
    fn invalid_folders_are_rejected_and_nothing_is_saved() {
        let mut conn = migrated();
        let before = Settings {
            theme: Theme::Light,
            ..Settings::default()
        };
        save(&mut conn, &before).unwrap();

        for bad in ["", "   ", "meetings/here", "nul\0byte"] {
            let attempt = Settings {
                storage_folder: Some(bad.to_string()),
                theme: Theme::Dark,
                consent_reminder: false,
                hotkeys: Hotkeys::default(),
            };
            let error = save(&mut conn, &attempt).unwrap_err();
            assert!(
                matches!(
                    error,
                    AppError::InvalidSetting {
                        field: "storageFolder",
                        ..
                    }
                ),
                "{bad:?} gave {error:?}"
            );
        }

        assert_eq!(
            load(&conn).unwrap(),
            before,
            "a rejected save must not change other values"
        );
    }

    #[test]
    fn an_unreadable_stored_value_falls_back_to_its_default() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('theme', '\"neon\"'), ('consent_reminder', '42')",
            [],
        )
        .unwrap();

        let settings = load(&conn).unwrap();

        assert_eq!(settings.theme, Theme::System);
        assert!(settings.consent_reminder);
    }

    #[test]
    fn json_shape_matches_what_the_frontend_expects() {
        let json = serde_json::to_value(Settings {
            storage_folder: None,
            theme: Theme::Dark,
            consent_reminder: true,
            hotkeys: Hotkeys::default(),
        })
        .unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "storageFolder": null,
                "theme": "dark",
                "consentReminder": true,
                "hotkeys": { "startStop": "Ctrl+Shift+S", "pauseResume": "Ctrl+Shift+P" }
            })
        );
    }

    #[test]
    fn the_default_shortcuts_are_ctrl_shift_s_and_p() {
        let hotkeys = Settings::default().hotkeys;

        assert_eq!(hotkeys.start_stop, "Ctrl+Shift+S");
        assert_eq!(hotkeys.pause_resume, "Ctrl+Shift+P");
    }

    #[test]
    fn shortcuts_are_trimmed_and_bad_ones_are_refused_with_the_field_named() {
        let mut conn = migrated();
        let trimmed = save(
            &mut conn,
            &Settings {
                hotkeys: Hotkeys {
                    start_stop: "  Ctrl+Alt+R ".into(),
                    pause_resume: "Ctrl+Alt+P".into(),
                },
                ..Settings::default()
            },
        )
        .unwrap();
        assert_eq!(trimmed.hotkeys.start_stop, "Ctrl+Alt+R");

        let mut refused = |start_stop: &str, pause_resume: &str| {
            save(
                &mut conn,
                &Settings {
                    hotkeys: Hotkeys {
                        start_stop: start_stop.into(),
                        pause_resume: pause_resume.into(),
                    },
                    ..Settings::default()
                },
            )
            .unwrap_err()
        };
        assert!(matches!(
            refused("", "Ctrl+Alt+P"),
            AppError::InvalidSetting {
                field: "hotkeys.startStop",
                ..
            }
        ));
        assert!(matches!(
            refused("Ctrl+Alt+R", "ctrl+alt+r"),
            AppError::InvalidSetting {
                field: "hotkeys.pauseResume",
                ..
            }
        ));
        assert!(matches!(
            refused("Ctrl+Alt+R", "Ctrl+\u{7}"),
            AppError::InvalidSetting {
                field: "hotkeys.pauseResume",
                ..
            }
        ));
    }
}
