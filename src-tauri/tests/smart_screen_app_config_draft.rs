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
    let draft = json!({"environment":"pre","switchEnvironment":false,"readyMode":"keep","homeMode":"off","voiceMode":"off","edits":{"h5Url":{"mode":"set","value":"https://example.org"}},"names":{},"targetIds":[screen],"includedIds":[screen],"rows":[]});
    app_config::save_draft(&state, &first, Some(draft))
        .await
        .unwrap();
    assert!(
        app_config::load_draft(&state, &first)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(app_config::load_draft(&state, &first).await.unwrap().unwrap()["homeMode"], "off");
    assert_eq!(app_config::load_draft(&state, &first).await.unwrap().unwrap()["voiceMode"], "off");
    assert!(app_config::save_draft(&state, &first, Some(json!({"targetIds":[],"rows":[],"homeMode":"invalid"}))).await.is_err());
    for invalid in [json!(null), json!(false), json!("false"), json!("invalid")] {
        assert!(app_config::save_draft(&state, &first, Some(json!({"targetIds":[],"rows":[],"voiceMode":invalid}))).await.is_err());
    }
    for mode in ["keep", "on", "off"] {
        app_config::save_draft(&state, &first, Some(json!({"targetIds":[screen],"rows":[],"voiceMode":mode}))).await.unwrap();
        assert_eq!(app_config::load_draft(&state, &first).await.unwrap().unwrap()["voiceMode"], mode);
    }
    app_config::save_draft(&state, &first, Some(json!({"targetIds":[screen],"rows":[]}))).await.unwrap();
    assert!(app_config::load_draft(&state, &first).await.unwrap().unwrap().get("voiceMode").is_none());
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

#[tokio::test]
async fn voice_leaf_draft_survives_reload_without_repeating_schema_or_creating_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let state = support::state_at(dir.path(), false).await;
    let project = Stage75Adapter::new(&state).create_project(support::local_input()).await.unwrap().project.id;
    let capabilities: serde_json::Value = serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../test/fixtures/app-config-schema-v1/schema-capabilities-small.json"))).unwrap();
    let read: serde_json::Value = serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../test/fixtures/app-config-schema-v1/schema-read-small.json"))).unwrap();
    let edits = json!({"autoStartVoiceEngine":{"mode":"set","value":false},"audioConfig.minSilenceDuration":{"mode":"set","value":0.62},"audioConfig.keywordsScore":{"mode":"keep"}});
    let draft = json!({"targetIds":["fixture"],"includedIds":["fixture"],"voiceMode":"off","voiceEdits":edits,"rows":[{"screenId":"fixture","capabilities":capabilities["data"],"config":read["data"]}]});
    app_config::save_draft(&state, &project, Some(draft)).await.unwrap();
    let saved = app_config::load_draft(&state, &project).await.unwrap().unwrap();
    assert_eq!(saved["voiceEdits"], edits);
    assert_eq!(saved["voiceMode"], "off");
    assert_eq!(saved["rows"][0]["config"]["audioConfig"]["micSampleRate"], 11025);
    assert!(saved["rows"][0]["capabilities"].get("configSchema").is_none());
    assert_eq!(saved["rows"][0]["capabilities"]["supportedFields"], capabilities["data"]["supportedFields"]);
    let malformed = json!({"targetIds":["fixture"],"rows":[],"voiceEdits":{"authCode":{"mode":"set","value":"do-not-store"}}});
    assert!(app_config::save_draft(&state, &project, Some(malformed)).await.is_err());
    assert_eq!(app_config::load_draft(&state, &project).await.unwrap().unwrap(), saved);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_task").fetch_one(state.local_store.pool()).await.unwrap();
    assert_eq!(count, 0);
    support::close(state).await;
}

#[tokio::test]
async fn home_activation_is_an_optional_boolean_draft_intent_without_device_or_task_action() {
    let dir = tempfile::tempdir().unwrap();
    let state = support::state_at(dir.path(), false).await;
    let project = Stage75Adapter::new(&state).create_project(support::local_input()).await.unwrap().project.id;
    let legacy = json!({"targetIds":[],"rows":[],"homeMode":"keep"});
    app_config::save_draft(&state, &project, Some(legacy)).await.unwrap();
    assert!(app_config::load_draft(&state, &project).await.unwrap().unwrap().get("homeActivation").is_none());
    for enabled in [true, false] {
        app_config::save_draft(&state, &project, Some(json!({"targetIds":[],"rows":[],"homeMode":"keep","homeActivation":enabled}))).await.unwrap();
        assert_eq!(app_config::load_draft(&state, &project).await.unwrap().unwrap()["homeActivation"], enabled);
    }
    let saved = app_config::load_draft(&state, &project).await.unwrap().unwrap();
    for invalid in [json!(null), json!("true"), json!(1), json!({}), json!([])] {
        assert!(app_config::save_draft(&state, &project, Some(json!({"targetIds":[],"rows":[],"homeActivation":invalid}))).await.is_err());
        assert_eq!(app_config::load_draft(&state, &project).await.unwrap().unwrap(), saved);
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_task").fetch_one(state.local_store.pool()).await.unwrap();
    assert_eq!(count, 0);
    support::close(state).await;
}
