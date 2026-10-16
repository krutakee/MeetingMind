//! Structured logging to rotating files in the app data folder.
//!
//! Privacy rule (SPEC Section 10): logs never contain transcript text. Log counts, durations, ids and error kinds,
//! never the words themselves. `tests/log_guard.rs` fails the build if a log macro mentions transcript text.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

use crate::error::AppError;

/// Log files kept on disk: one per day, oldest deleted first.
const MAX_LOG_FILES: usize = 14;

/// Environment variable that overrides the log filter, for example `MEETINGMIND_LOG=debug`.
const FILTER_ENV: &str = "MEETINGMIND_LOG";

/// Keeps the background log writer alive. Dropping it flushes pending log lines.
pub struct LogGuard {
    _worker: WorkerGuard,
}

/// Starts JSON-lines logging into `log_dir` (created if missing). Debug builds also log to the console.
/// Can only succeed once per process.
pub fn init(log_dir: &Path) -> Result<LogGuard, AppError> {
    std::fs::create_dir_all(log_dir)?;

    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("meetingmind")
        .filename_suffix("log")
        .max_log_files(MAX_LOG_FILES)
        .build(log_dir)
        .map_err(|error| AppError::Logging(error.to_string()))?;
    let (writer, worker) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_env(FILTER_ENV).unwrap_or_else(|_| EnvFilter::new("info"));
    let file_layer = fmt::layer().json().with_ansi(false).with_writer(writer);
    let console_layer = cfg!(debug_assertions).then(|| fmt::layer().with_writer(std::io::stderr));

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .try_init()
        .map_err(|error| AppError::Logging(error.to_string()))?;

    install_panic_hook();

    Ok(LogGuard { _worker: worker })
}

/// Records where a panic happened. The panic message is left out of the log file on purpose: it can contain
/// arbitrary data, including transcript text. The default hook still prints the full message to stderr.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "unknown".to_string());
        tracing::error!(location, "panic (message not logged)");
        default_hook(info);
    }));
}
