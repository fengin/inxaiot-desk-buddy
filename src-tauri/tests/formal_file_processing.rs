use std::collections::BTreeMap;
use std::sync::Mutex;

use inxaiot_desk_buddy_lib::application::ports::file_processing::{
    FileProcessingProgress, FileProcessingProgressSink, FileProcessingService, FileProcessingStage,
};
use inxaiot_desk_buddy_lib::core::error::{AppError, AppResult};
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::infrastructure::file_processing::LocalFileProcessor;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::recent_artifact_repository::RecentArtifactRepository;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct ProgressRecorder {
    last: Mutex<BTreeMap<FileProcessingStage, u64>>,
}

impl FileProcessingProgressSink for ProgressRecorder {
    fn emit(&self, progress: FileProcessingProgress) -> AppResult<()> {
        let mut values = self.last.lock().expect("progress lock");
        let previous = values.get(&progress.stage).copied().unwrap_or_default();
        assert!(progress.processed >= previous);
        assert!(progress.processed <= progress.total);
        values.insert(progress.stage, progress.processed);
        Ok(())
    }
}

#[tokio::test]
async fn selected_file_hash_workspace_copy_cancel_and_exact_cleanup_are_bounded() {
    let temp = tempfile::tempdir().expect("temporary file root");
    let selected = temp.path().join("selected");
    let outside = temp.path().join("outside");
    tokio::fs::create_dir_all(&selected)
        .await
        .expect("selected directory");
    tokio::fs::create_dir_all(&outside)
        .await
        .expect("outside directory");
    let bytes = vec![0x5a; 2 * 1024 * 1024];
    let source = selected.join("release.bin");
    let outside_file = outside.join("outside.bin");
    tokio::fs::write(&source, &bytes)
        .await
        .expect("source file");
    tokio::fs::write(&outside_file, b"outside")
        .await
        .expect("outside file");
    let processor = LocalFileProcessor::open(temp.path().join("workspaces"))
        .await
        .expect("file processor");
    let progress = ProgressRecorder::default();
    let descriptor = processor
        .inspect_selected_file(&selected, &source, &CancellationToken::new(), &progress)
        .await
        .expect("inspect selected file");
    assert_eq!(descriptor.size, bytes.len() as u64);
    assert_eq!(descriptor.sha256, hex::encode(Sha256::digest(&bytes)));
    assert_eq!(
        progress
            .last
            .lock()
            .expect("progress lock")
            .get(&FileProcessingStage::Hashing)
            .copied(),
        Some(bytes.len() as u64)
    );
    assert!(
        processor
            .inspect_selected_file(
                &selected,
                &outside_file,
                &CancellationToken::new(),
                &ProgressRecorder::default(),
            )
            .await
            .is_err()
    );

    let workspace = processor
        .create_workspace("project-a", "task-a")
        .await
        .expect("create workspace");
    assert!(
        processor
            .create_workspace("project-a", "task-a")
            .await
            .is_err()
    );
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(matches!(
        processor
            .copy_to_workspace(
                &descriptor,
                &workspace,
                "release.bin",
                &cancellation,
                &ProgressRecorder::default(),
            )
            .await,
        Err(AppError::Cancelled)
    ));
    assert!(!workspace.temporary_dir.join("release.bin.part").exists());
    let copied = processor
        .copy_to_workspace(
            &descriptor,
            &workspace,
            "release.bin",
            &CancellationToken::new(),
            &ProgressRecorder::default(),
        )
        .await
        .expect("copy verified file");
    assert_eq!(copied.size, descriptor.size);
    assert_eq!(copied.sha256, descriptor.sha256);
    assert!(copied.path.starts_with(&workspace.artifacts_dir));
    processor
        .cleanup_workspace(&workspace)
        .await
        .expect("cleanup exact workspace");
    assert!(!workspace.root.exists());
    assert!(processor.workspace_root().exists());
}

#[tokio::test]
async fn recent_artifact_is_local_project_and_business_domain_scoped() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    sqlx::query(
        "INSERT INTO local_project \
         (id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, \
          db_password_secret_ref, created_at, updated_at) \
         VALUES ('project-a', 'Project A', 'http://platform.test', 'db.test', 3306, \
                 'user', 'business', 'workbench', 'secret-ref', '1', '1')",
    )
    .execute(store.pool())
    .await
    .expect("project fixture");
    let repository = RecentArtifactRepository::new(store.pool().clone());
    let first = temp.path().join("first.tar");
    let second = temp.path().join("second.tar");
    repository
        .save("project-a", "aio", "image", "device-edge", first)
        .await
        .expect("save first recent file");
    repository
        .save("project-a", "aio", "image", "device-edge", second.clone())
        .await
        .expect("replace recent file");
    let record = repository
        .get("project-a", "aio", "image", "device-edge")
        .await
        .expect("read recent file")
        .expect("recent file exists");
    assert_eq!(record.path, second);
    assert!(
        repository
            .get("project-a", "gateway", "image", "device-edge")
            .await
            .expect("read other domain")
            .is_none()
    );
    store.close().await;
}
