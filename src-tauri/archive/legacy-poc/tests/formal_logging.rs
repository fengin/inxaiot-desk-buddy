#[path = "../src/formal/error.rs"]
mod error;
#[path = "../src/formal/logging.rs"]
mod logging;

#[test]
fn structured_file_logging_writes_without_ansi_or_secrets() {
    let temp = tempfile::tempdir().expect("temporary log directory");
    let guard = logging::init_file_logging(temp.path()).expect("initialize file logging");
    tracing::info!(
        event_code = "FOUNDATION_TEST",
        project_id = "project-a",
        "formal logging test"
    );
    drop(guard);

    let entries = std::fs::read_dir(temp.path())
        .expect("read log directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert!(!entries.is_empty());
    let content = entries
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .collect::<String>();
    assert!(content.contains("FOUNDATION_TEST"));
    assert!(content.contains("project-a"));
    assert!(!content.contains("\u{1b}["));
}

