use std::sync::Arc;

use inxaiot_desk_buddy_lib::domain::common::task::TaskState;
use inxaiot_desk_buddy_lib::formal::error::FormalError;
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::formal::project_repository::{
    CreateLocalProject, LocalProjectRepository, UpdateLocalProject,
};
use inxaiot_desk_buddy_lib::formal::secret_store::MemorySecretStore;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::task_repository::{
    CreateTask, TaskRepository,
};

fn input(name: &str, database_password: &str) -> CreateLocalProject {
    CreateLocalProject {
        name: name.into(),
        platform_url: "http://platform.test:8055".into(),
        db_host: "database.test".into(),
        db_port: 3306,
        db_user: "workbench".into(),
        db_password: database_password.into(),
        business_db: "inxvision_iot_dev".into(),
        workbench_db: "inxaiot_desk_buddy".into(),
    }
}

fn update(name: &str, platform_url: &str, database_password: Option<&str>) -> UpdateLocalProject {
    UpdateLocalProject {
        name: name.into(),
        platform_url: platform_url.into(),
        db_host: "database-updated.test".into(),
        db_port: 3307,
        db_user: "workbench-updated".into(),
        db_password: database_password.map(str::to_string),
        business_db: "inxvision_iot_dev".into(),
        workbench_db: "inxaiot_desk_buddy".into(),
    }
}

#[tokio::test]
async fn projects_and_sessions_are_isolated_and_secrets_stay_out_of_sqlite() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    let secrets = Arc::new(MemorySecretStore::default());
    let repository = LocalProjectRepository::new(store.pool().clone(), secrets);

    let first = repository
        .create(input("Project A", "password-a"))
        .await
        .expect("create project a");
    let second = repository
        .create(input("Project B", "password-b"))
        .await
        .expect("create project b");
    assert_ne!(first.id, second.id);
    assert_eq!(repository.list().await.expect("list projects").len(), 2);

    let first_secrets = repository
        .connection_secrets(&first.id)
        .await
        .expect("project a secrets");
    let second_secrets = repository
        .connection_secrets(&second.id)
        .await
        .expect("project b secrets");
    assert_eq!(first_secrets.db_password, "password-a");
    assert_eq!(second_secrets.db_password, "password-b");
    assert!(!first_secrets.to_string().contains("password-a"));

    repository
        .save_session(&first.id, "user-a", "token-a", None)
        .await
        .expect("save session a");
    repository
        .save_session(&second.id, "user-b", "token-b", None)
        .await
        .expect("save session b");
    let session_a = repository.load_session(&first.id).await.expect("session a");
    let session_b = repository
        .load_session(&second.id)
        .await
        .expect("session b");
    assert_eq!(session_a.access_token, "token-a");
    assert_eq!(session_b.access_token, "token-b");

    let project_row = sqlx::query_as::<_, (String, String)>(
        "SELECT db_password_secret_ref, name FROM local_project WHERE id = ?",
    )
    .bind(&first.id)
    .fetch_one(store.pool())
    .await
    .expect("read project row");
    assert!(!project_row.0.contains("password-a"));
    let local_db_bytes = std::fs::read(temp.path().join("local.db")).expect("read sqlite file");
    let local_db_text = String::from_utf8_lossy(&local_db_bytes);
    assert!(!local_db_text.contains("password-a"));
    assert!(!local_db_text.contains("token-a"));

    repository
        .delete(&first.id)
        .await
        .expect("delete project a");
    assert!(repository.get(&first.id).await.is_err());
    assert!(repository.get(&second.id).await.is_ok());
    store.close().await;
}

#[tokio::test]
async fn invalid_project_input_is_rejected_before_secret_write() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    let repository =
        LocalProjectRepository::new(store.pool().clone(), Arc::new(MemorySecretStore::default()));
    let mut invalid = input("", "password");
    invalid.db_port = 0;
    assert!(repository.create(invalid).await.is_err());
    assert!(repository.list().await.expect("list projects").is_empty());
    store.close().await;
}

#[tokio::test]
async fn project_update_keeps_or_rotates_secret_and_invalidates_changed_platform_session() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    let repository =
        LocalProjectRepository::new(store.pool().clone(), Arc::new(MemorySecretStore::default()));
    let project = repository
        .create(input("Project A", "password-a"))
        .await
        .expect("create project");
    repository
        .save_session(&project.id, "user-a", "token-a", None)
        .await
        .expect("save session");

    repository
        .update(
            &project.id,
            update("Project A2", "http://platform.test:8055", None),
        )
        .await
        .expect("update without password");
    assert_eq!(
        repository
            .connection_secrets(&project.id)
            .await
            .expect("unchanged secret")
            .db_password,
        "password-a"
    );
    assert!(repository.load_session(&project.id).await.is_ok());

    repository
        .update(
            &project.id,
            update(
                "Project A3",
                "http://platform-new.test:8055",
                Some("password-b"),
            ),
        )
        .await
        .expect("update with password and platform");
    assert_eq!(
        repository
            .connection_secrets(&project.id)
            .await
            .expect("rotated secret")
            .db_password,
        "password-b"
    );
    assert!(repository.load_session(&project.id).await.is_err());

    repository
        .save_session(&project.id, "user-b", "token-b", None)
        .await
        .expect("save new session");
    repository
        .clear_session(&project.id)
        .await
        .expect("clear session");
    assert!(repository.load_session(&project.id).await.is_err());
    store.close().await;
}

#[tokio::test]
async fn active_task_blocks_project_update_and_delete_at_the_sqlite_boundary() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    let repository =
        LocalProjectRepository::new(store.pool().clone(), Arc::new(MemorySecretStore::default()));
    let project = repository
        .create(input("Protected Project", "password-a"))
        .await
        .expect("create project");
    let tasks = TaskRepository::new(store.pool().clone());
    tasks
        .create(CreateTask {
            id: "active-project-task".into(),
            local_project_id: project.id.clone(),
            remote_operation_record_id: None,
            domain_type: "aio".into(),
            operation_type: "full_upgrade".into(),
            name: "Protected deployment".into(),
            priority: 0,
            batch_size: 1,
            concurrency: 1,
            payload_ref: Some("payload".into()),
            log_path: "task.jsonl".into(),
            targets: vec![("aio".into(), "001122334455".into())],
        })
        .await
        .expect("create task");
    for (expected, next) in [
        (TaskState::Draft, TaskState::Checking),
        (TaskState::Checking, TaskState::Ready),
        (TaskState::Ready, TaskState::Queued),
    ] {
        tasks
            .transition("active-project-task", expected, next, None, None)
            .await
            .expect("advance task");
    }

    assert!(
        tasks
            .has_active_for_project(&project.id)
            .await
            .expect("active project query")
    );
    assert!(matches!(
        repository
            .update(
                &project.id,
                update("Changed", "http://platform.test:8055", None)
            )
            .await,
        Err(FormalError::Conflict(_))
    ));
    assert!(matches!(
        repository.delete(&project.id).await,
        Err(FormalError::Conflict(_))
    ));
    assert!(repository.get(&project.id).await.is_ok());

    tasks
        .transition(
            "active-project-task",
            TaskState::Queued,
            TaskState::Cancelled,
            None,
            Some("test completed"),
        )
        .await
        .expect("finish task");
    assert!(
        !tasks
            .has_active_for_project(&project.id)
            .await
            .expect("terminal project query")
    );
    repository
        .delete(&project.id)
        .await
        .expect("delete terminal project");
    store.close().await;
}
