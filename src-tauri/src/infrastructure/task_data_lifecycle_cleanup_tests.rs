use std::fs;
use std::path::{Path, PathBuf};

use super::{TaskDataLifecycle, retention_path};
use crate::domain::common::task::TaskState;
use crate::formal::config::AppPaths;

fn fixture() -> (tempfile::TempDir, AppPaths) {
    let root = tempfile::tempdir().expect("isolated root");
    let paths = AppPaths::from_data_dir(root.path().join("instance")).expect("paths");
    paths.ensure().expect("directories");
    (root, paths)
}

fn artifact(paths: &AppPaths, task: &str, relative: &str) -> PathBuf {
    let path = paths
        .project_task_dir("project", task)
        .expect("task directory")
        .join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("parents");
    fs::write(&path, relative.as_bytes()).expect("fixture file");
    path
}

#[test]
fn installed_apk_is_removed_without_deleting_diagnostics_metadata_source_or_logs() {
    let (root, paths) = fixture();
    let original = root.path().join("original.apk");
    fs::write(&original, b"user selected source").unwrap();
    let apk = artifact(&paths, "installed", "xiaoxin.apk");
    let diagnostic = artifact(&paths, "installed", "diagnostic-device.json");
    let metadata = artifact(&paths, "installed", "package-metadata.json");
    let sibling = artifact(&paths, "still-running", "xiaoxin.apk");
    let log = paths.project_task_log_path("project", "installed").unwrap();
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, b"persistent evidence").unwrap();
    let lifecycle = TaskDataLifecycle::new(&paths);

    lifecycle
        .finalize_task_for_operation(
            "project",
            "installed",
            TaskState::Succeeded,
            "smart_screen",
            "install",
        )
        .unwrap();
    assert!(!apk.exists());
    assert!(diagnostic.is_file() && metadata.is_file() && sibling.is_file());
    assert_eq!(fs::read(original).unwrap(), b"user selected source");
    assert_eq!(fs::read(&log).unwrap(), b"persistent evidence");
    let retention = fs::read(retention_path(&log)).unwrap();
    lifecycle
        .finalize_task_for_operation(
            "project",
            "installed",
            TaskState::Succeeded,
            "smart_screen",
            "install",
        )
        .unwrap();
    assert_eq!(fs::read(retention_path(&log)).unwrap(), retention);
}

#[test]
fn diagnostic_and_unknown_operations_keep_all_artifacts() {
    let (_root, paths) = fixture();
    let lifecycle = TaskDataLifecycle::new(&paths);
    for (task, domain, operation) in [
        ("diagnostic", "smart_screen", "diagnostics"),
        ("future-screen", "smart_screen", "future-operation"),
        ("future-aio", "aio", "future-operation"),
        ("future-domain", "future-domain", "install"),
    ] {
        let apk = artifact(&paths, task, "xiaoxin.apk");
        let diagnostic = artifact(&paths, task, "diagnostic-device.json");
        lifecycle
            .finalize_task_for_operation("project", task, TaskState::Succeeded, domain, operation)
            .unwrap();
        assert!(apk.is_file() && diagnostic.is_file());
    }
}

#[test]
fn running_cancelling_and_unconfirmed_tasks_keep_packages_and_recovery_evidence() {
    let (_root, paths) = fixture();
    let lifecycle = TaskDataLifecycle::new(&paths);
    for state in [
        TaskState::Queued,
        TaskState::Running,
        TaskState::Cancelling,
        TaskState::FinalizingFailed,
    ] {
        let task = state.as_str();
        let package = artifact(&paths, task, "xiaoxin.apk");
        assert!(
            lifecycle
                .finalize_task_for_operation("project", task, state, "smart_screen", "install")
                .is_err()
        );
        assert!(package.is_file());
    }
    let package = artifact(&paths, "interrupted", "release.tar");
    let recovery = artifact(&paths, "interrupted", "pending-local-finalization.json");
    lifecycle
        .finalize_task_for_operation(
            "project",
            "interrupted",
            TaskState::Interrupted,
            "aio",
            "full_upgrade",
        )
        .unwrap();
    assert!(package.is_file() && recovery.is_file());
    lifecycle
        .finalize_task_for_operation(
            "project",
            "interrupted",
            TaskState::Succeeded,
            "aio",
            "full_upgrade",
        )
        .unwrap();
    assert!(!package.exists() && !recovery.exists());
}

#[test]
fn aio_terminal_cleanup_still_removes_both_image_snapshots_and_release_archive() {
    let (_root, paths) = fixture();
    let lifecycle = TaskDataLifecycle::new(&paths);
    for (task, operation, state) in [
        ("first", "first_deploy", TaskState::Succeeded),
        ("full", "full_upgrade", TaskState::PartiallySucceeded),
        ("single", "service_upgrade", TaskState::Failed),
        ("cancelled", "service_upgrade", TaskState::Cancelled),
    ] {
        artifact(&paths, task, "generated-release/images/service.tar");
        artifact(&paths, task, "release.tar");
        artifact(&paths, task, "service-image.tar");
        lifecycle
            .finalize_task_for_operation("project", task, state, "aio", operation)
            .unwrap();
        assert!(!paths.project_task_dir("project", task).unwrap().exists());
    }
}

#[test]
fn interrupted_aio_without_finalization_file_keeps_artifacts_and_log_evidence() {
    let (_root, paths) = fixture();
    let package = artifact(&paths, "crashed-before-record", "release.tar");
    let image = artifact(
        &paths,
        "crashed-before-record",
        "generated-release/images/app.tar",
    );
    let log = paths
        .project_task_log_path("project", "crashed-before-record")
        .unwrap();
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, b"last known remote operation").unwrap();
    let lifecycle = TaskDataLifecycle::new(&paths);
    lifecycle
        .finalize_task_for_operation(
            "project",
            "crashed-before-record",
            TaskState::Interrupted,
            "aio",
            "full_upgrade",
        )
        .unwrap();
    lifecycle
        .finalize_task("project", "crashed-before-record", TaskState::Interrupted)
        .unwrap();
    assert!(package.is_file() && image.is_file());
    assert_eq!(fs::read(&log).unwrap(), b"last known remote operation");
    assert!(!retention_path(&log).exists());
}

#[test]
fn interrupted_aio_removes_legacy_expired_marker_and_survives_log_sweep() {
    let (_root, paths) = fixture();
    let package = artifact(&paths, "legacy-interrupted", "release.tar");
    let log = paths
        .project_task_log_path("project", "legacy-interrupted")
        .unwrap();
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, b"unconfirmed remote result").unwrap();
    fs::write(
        retention_path(&log),
        br#"{"taskId":"legacy-interrupted","taskState":"interrupted","deleteAfterEpochSeconds":1}"#,
    )
    .unwrap();
    let lifecycle = TaskDataLifecycle::new(&paths);
    lifecycle
        .finalize_task_for_operation(
            "project",
            "legacy-interrupted",
            TaskState::Interrupted,
            "aio",
            "full_upgrade",
        )
        .unwrap();
    assert!(!retention_path(&log).exists());
    assert_eq!(lifecycle.sweep_expired_logs().unwrap(), 0);
    assert_eq!(fs::read(&log).unwrap(), b"unconfirmed remote result");
    assert!(package.is_file());
    lifecycle
        .protect_task_log("project", "legacy-interrupted")
        .unwrap();
}

#[test]
fn orphan_cleanup_is_limited_to_known_large_files_and_preserves_recovery_tasks() {
    let (_root, paths) = fixture();
    let lifecycle = TaskDataLifecycle::new(&paths);
    let apk = artifact(&paths, "orphan-screen", "xiaoxin.apk");
    let diagnostic = artifact(&paths, "orphan-screen", "diagnostic-device.json");
    let known_images = [
        "release.tar",
        "service-image.tar",
        "service-image.tar.part",
        "generated-release/images/service.tar",
        "generated-release.part/images/service.tar",
    ]
    .map(|name| artifact(&paths, "orphan-aio", name));
    let metadata = artifact(&paths, "orphan-aio", "generated-release/manifest.json");
    let tag = artifact(&paths, "orphan-aio", "generated-release/images/service.tag");
    let unknown = artifact(&paths, "orphan-aio", "unknown/place/image.tar");
    let protected = artifact(&paths, "pending", "release.tar");
    let recovery = artifact(&paths, "pending", "pending-local-finalization.json");
    artifact(&paths, "diagnostics-only", "diagnostic-device.json");
    let mut candidates = lifecycle.list_orphan_artifact_candidates().unwrap();
    candidates.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    assert_eq!(
        candidates
            .iter()
            .map(|value| value.task_id.as_str())
            .collect::<Vec<_>>(),
        ["orphan-aio", "orphan-screen"]
    );

    assert_eq!(
        lifecycle
            .clear_orphan_artifacts("project", "orphan-screen")
            .unwrap(),
        1
    );
    assert_eq!(
        lifecycle
            .clear_orphan_artifacts("project", "orphan-aio")
            .unwrap(),
        known_images.len()
    );
    assert_eq!(
        lifecycle
            .clear_orphan_artifacts("project", "pending")
            .unwrap(),
        0
    );
    assert_eq!(
        lifecycle
            .clear_orphan_artifacts("project", "orphan-aio")
            .unwrap(),
        0
    );
    assert!(!apk.exists() && known_images.iter().all(|path| !path.exists()));
    assert!(
        [diagnostic, metadata, tag, unknown, protected, recovery]
            .iter()
            .all(|path| path.is_file())
    );
}

#[test]
fn cleanup_rejects_path_traversal_and_windows_drive_relative_ids() {
    let (_root, paths) = fixture();
    let lifecycle = TaskDataLifecycle::new(&paths);
    let package = artifact(&paths, "valid", "xiaoxin.apk");
    for invalid in [
        "..",
        "../outside",
        "C:outside",
        "project\\outside",
        "project.",
        "project ",
    ] {
        assert!(lifecycle.clear_orphan_artifacts(invalid, "valid").is_err());
        assert!(
            lifecycle
                .finalize_task_for_operation(
                    "project",
                    invalid,
                    TaskState::Succeeded,
                    "smart_screen",
                    "install"
                )
                .is_err()
        );
    }
    assert!(package.is_file());
}

#[cfg(any(windows, unix))]
fn directory_link(target: &Path, link: &Path) {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("create isolated junction");
        assert!(
            output.status.success(),
            "junction fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("create isolated directory link");
}

#[cfg(any(windows, unix))]
#[test]
fn linked_task_directory_and_linked_aio_subtree_never_delete_external_files() {
    let (root, paths) = fixture();
    let outside = root.path().join("external");
    fs::create_dir(&outside).unwrap();
    let external_package = outside.join("xiaoxin.apk");
    fs::write(&external_package, b"not managed").unwrap();
    let project = paths.task_artifacts_dir.join("project");
    fs::create_dir_all(&project).unwrap();
    let linked_task = project.join("linked-task");
    directory_link(&outside, &linked_task);
    let lifecycle = TaskDataLifecycle::new(&paths);
    assert!(
        lifecycle
            .finalize_task_for_operation(
                "project",
                "linked-task",
                TaskState::Succeeded,
                "smart_screen",
                "install"
            )
            .is_err()
    );
    assert!(
        lifecycle
            .clear_orphan_artifacts("project", "linked-task")
            .is_err()
    );
    assert!(
        lifecycle
            .list_orphan_artifact_candidates()
            .unwrap()
            .is_empty()
    );
    assert_eq!(fs::read(&external_package).unwrap(), b"not managed");

    let managed = artifact(&paths, "aio-with-link", "release.tar");
    let linked_subtree = managed.parent().unwrap().join("images-link");
    directory_link(&outside, &linked_subtree);
    assert!(
        lifecycle
            .finalize_task_for_operation(
                "project",
                "aio-with-link",
                TaskState::Succeeded,
                "aio",
                "full_upgrade"
            )
            .is_err()
    );
    assert!(managed.is_file());
    assert_eq!(fs::read(&external_package).unwrap(), b"not managed");
    // 删除的只是本测试创建的链接节点，目标目录继续保留。
    #[cfg(windows)]
    {
        fs::remove_dir(linked_subtree).unwrap();
        fs::remove_dir(linked_task).unwrap();
    }
    #[cfg(unix)]
    {
        fs::remove_file(linked_subtree).unwrap();
        fs::remove_file(linked_task).unwrap();
    }
}

#[cfg(windows)]
#[test]
fn occupied_apk_keeps_log_retention_and_can_be_cleaned_on_next_attempt() {
    use std::os::windows::fs::OpenOptionsExt;

    let (_root, paths) = fixture();
    let apk = artifact(&paths, "occupied", "xiaoxin.apk");
    let log = paths.project_task_log_path("project", "occupied").unwrap();
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, b"evidence").unwrap();
    let handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&apk)
        .unwrap();
    let lifecycle = TaskDataLifecycle::new(&paths);
    assert!(
        lifecycle
            .finalize_task_for_operation(
                "project",
                "occupied",
                TaskState::Succeeded,
                "smart_screen",
                "install"
            )
            .is_err()
    );
    assert!(apk.is_file() && log.is_file() && retention_path(&log).is_file());
    drop(handle);
    lifecycle
        .finalize_task_for_operation(
            "project",
            "occupied",
            TaskState::Succeeded,
            "smart_screen",
            "install",
        )
        .unwrap();
    assert!(!apk.exists());
    assert_eq!(fs::read(log).unwrap(), b"evidence");
}

#[cfg(windows)]
#[test]
fn occupied_legacy_retention_marker_reports_failure_without_deleting_log() {
    use std::os::windows::fs::OpenOptionsExt;

    let (_root, paths) = fixture();
    let log = paths
        .project_task_log_path("project", "occupied-marker")
        .unwrap();
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, b"unconfirmed evidence").unwrap();
    let marker = retention_path(&log);
    fs::write(
        &marker,
        br#"{"taskId":"occupied-marker","taskState":"interrupted","deleteAfterEpochSeconds":1}"#,
    )
    .unwrap();
    let handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&marker)
        .unwrap();
    let lifecycle = TaskDataLifecycle::new(&paths);
    assert!(
        lifecycle
            .protect_task_log("project", "occupied-marker")
            .is_err()
    );
    assert!(marker.is_file());
    assert_eq!(fs::read(&log).unwrap(), b"unconfirmed evidence");
    drop(handle);
    lifecycle
        .protect_task_log("project", "occupied-marker")
        .unwrap();
    assert_eq!(lifecycle.sweep_expired_logs().unwrap(), 0);
    assert!(log.is_file());
}
