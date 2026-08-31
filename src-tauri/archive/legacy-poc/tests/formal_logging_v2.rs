#[path = "../src/formal/error.rs"]
mod error;
#[path = "../src/formal/logging.rs"]
mod logging;

use std::time::{Duration, Instant};

#[test]
fn structured_file_logging_flushes_to_disk() {
    let temp = tempfile::tempdir().expect("temporary log directory");
    let guard = logging::init_file_logging(temp.path()).expect("initialize file logging");
    tracing::info!(
        event_code = "FOUNDATION_TEST",
        project_id = "project-a",
        "formal logging test"
    );

    let deadline = Instant::now() + Duration::from_secs(2);
    let content = loop {
        let content = std::fs::read_dir(temp.path())
            .expect("read log directory")
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .collect::<String>();
        if content.contains("FOUNDATION_TEST") || Instant::now() >= deadline {
            break content;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(guard);
    assert!(content.contains("FOUNDATION_TEST"));
    assert!(content.contains("project-a"));
    assert!(!content.contains("\u{1b}["));
}

