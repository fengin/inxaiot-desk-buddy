#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::smart_screen::model::ScreenFields,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository, smart_screen::app_config,
        stage75_adapter::Stage75Adapter,
    },
};
use serde_json::json;
#[tokio::test]
async fn configuration_draft_is_local_project_scoped_and_never_locks_asset() {
    let dir = tempfile::tempdir().unwrap();
    let state = support::state_at(dir.path(), false).await;
    let adapter = Stage75Adapter::new(&state);
    let first = adapter
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let mut other = support::local_input();
    other.name = "另一项目".into();
    let second = adapter.create_project(other).await.unwrap().project.id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let screen = repo
        .save_local(
            &first,
            &ScreenFields {
                name: "草稿屏".into(),
                ip: "192.0.2.1".into(),
                size: "4".into(),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    let draft = json!({"environment":"pre","switchEnvironment":false,"readyMode":"keep","edits":{"h5Url":{"mode":"set","value":"https://example.org"}},"names":{},"targetIds":[screen],"includedIds":[screen],"rows":[]});
    app_config::save_draft(&state, &first, Some(draft))
        .await
        .unwrap();
    assert!(
        app_config::load_draft(&state, &first)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        app_config::load_draft(&state, &second)
            .await
            .unwrap()
            .is_none()
    );
    repo.require_idle(&first, &screen).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_task")
        .fetch_one(state.local_store.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    app_config::save_draft(&state, &second, None).await.unwrap();
    assert!(
        app_config::load_draft(&state, &first)
            .await
            .unwrap()
            .is_some()
    );
    sqlx::query("UPDATE local_screen_task_data SET plan_sha256='broken' WHERE local_project_id=?")
        .bind(&first)
        .execute(state.local_store.pool())
        .await
        .unwrap();
    assert!(app_config::load_draft(&state, &first).await.is_err());
    app_config::save_draft(&state, &first, None).await.unwrap();
    assert!(
        app_config::load_draft(&state, &first)
            .await
            .unwrap()
            .is_none()
    );
    support::close(state).await;
}
