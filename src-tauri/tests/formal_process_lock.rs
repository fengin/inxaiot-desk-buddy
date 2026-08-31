use inxaiot_desk_buddy_lib::formal::config::AppPaths;
use inxaiot_desk_buddy_lib::formal::error::FormalError;
use inxaiot_desk_buddy_lib::infrastructure::process_lock::DataDirectoryProcessLock;

const CHILD_DATA_DIRECTORY: &str = "INXAIOT_PROCESS_LOCK_CHILD_DATA_DIRECTORY";

#[test]
fn data_directory_lock_rejects_a_second_owner_and_recovers_after_drop() {
    let temp = tempfile::tempdir().expect("temporary data directory");
    let paths = AppPaths::from_data_dir(temp.path()).expect("app paths");
    paths.ensure().expect("create app paths");

    let first = DataDirectoryProcessLock::acquire(&paths.process_lock).expect("first owner");
    assert_eq!(first.path(), paths.process_lock.as_path());
    assert!(matches!(
        DataDirectoryProcessLock::acquire(&paths.process_lock),
        Err(FormalError::Conflict(_))
    ));

    drop(first);
    DataDirectoryProcessLock::acquire(&paths.process_lock)
        .expect("lock becomes available when the owning process handle closes");
}

#[test]
fn data_directory_lock_child_process() {
    let Some(data_directory) = std::env::var_os(CHILD_DATA_DIRECTORY) else {
        return;
    };
    let paths = AppPaths::from_data_dir(data_directory).expect("child app paths");
    paths.ensure().expect("child create app paths");
    let _lock = DataDirectoryProcessLock::acquire(&paths.process_lock).expect("child owns lock");
    std::fs::write(paths.data_dir.join("child-ready"), b"ready").expect("write child ready");
    for _ in 0..500 {
        if paths.data_dir.join("child-release").exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("parent did not release child process");
}

#[test]
fn data_directory_lock_rejects_a_real_second_process() {
    let temp = tempfile::tempdir().expect("temporary data directory");
    let paths = AppPaths::from_data_dir(temp.path()).expect("app paths");
    paths.ensure().expect("create app paths");
    let mut child = std::process::Command::new(std::env::current_exe().expect("current test exe"))
        .arg("--exact")
        .arg("data_directory_lock_child_process")
        .arg("--nocapture")
        .env(CHILD_DATA_DIRECTORY, &paths.data_dir)
        .spawn()
        .expect("spawn lock owner process");
    for _ in 0..500 {
        if paths.data_dir.join("child-ready").exists() {
            break;
        }
        if child.try_wait().expect("read child state").is_some() {
            panic!("lock owner process exited before becoming ready");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(paths.data_dir.join("child-ready").exists());
    assert!(matches!(
        DataDirectoryProcessLock::acquire(&paths.process_lock),
        Err(FormalError::Conflict(_))
    ));
    std::fs::write(paths.data_dir.join("child-release"), b"release")
        .expect("release child process");
    assert!(child.wait().expect("wait child process").success());
}
