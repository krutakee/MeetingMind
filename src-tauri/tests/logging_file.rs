//! Runs in its own process because `logging::init` installs the global logger and panic hook.

use std::fs;

use meetingmind_lib::logging;

#[test]
fn logs_go_to_a_json_file_and_panic_messages_stay_out_of_it() {
    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("logs");

    let guard = logging::init(&log_dir).expect("logging starts in an empty folder");
    tracing::info!(answer = 42, "hello from the logging test");

    // A panic must be logged by location only: its message can hold private data.
    let panicked = std::panic::catch_unwind(|| panic!("PRIVATE-PAYLOAD-12345"));
    assert!(panicked.is_err());

    // Starting a second logger in the same process is refused instead of silently replacing the first.
    let second = logging::init(&dir.path().join("logs-second"));
    assert!(second.is_err(), "a second init must fail");

    drop(guard); // flushes the background writer

    let files: Vec<_> = fs::read_dir(&log_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "one log file for today: {files:?}");
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("meetingmind") && name.ends_with(".log"),
        "unexpected log file name {name}"
    );

    let content = fs::read_to_string(&files[0]).unwrap();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .map(|line| serde_json::from_str(line).expect("every log line is valid JSON"))
        .collect();

    let hello = lines
        .iter()
        .find(|line| line["fields"]["message"] == "hello from the logging test")
        .expect("the info line was written");
    assert_eq!(hello["level"], "INFO");
    assert_eq!(hello["fields"]["answer"], 42);

    let panic_line = lines
        .iter()
        .find(|line| {
            line["level"] == "ERROR" && line["fields"]["message"] == "panic (message not logged)"
        })
        .expect("the panic was logged");
    assert!(
        panic_line["fields"]["location"]
            .as_str()
            .is_some_and(|location| location.contains("logging_file.rs")),
        "panic location missing: {panic_line}"
    );
    assert!(
        !content.contains("PRIVATE-PAYLOAD-12345"),
        "the panic message leaked into the log file"
    );
}
