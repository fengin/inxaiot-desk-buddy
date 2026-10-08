use inxaiot_desk_buddy_lib::domain::smart_screen::model::*;
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::screen_repository::{
    ScreenRepository, now,
};

async fn project(store: &LocalStore, id: &str) {
    sqlx::query("INSERT INTO local_project(id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) VALUES(?,?,'','',3306,'','','inxaiot_desk_buddy','',?,?)")
        .bind(id).bind(id).bind(now()).bind(now()).execute(store.pool()).await.unwrap();
}
fn fields() -> ScreenFields {
    ScreenFields {
        name: "测试屏".into(),
        ip: "192.168.3.114".into(),
        mac: String::new(),
        size: "4".into(),
        space_id: None,
        location: "桌面".into(),
    }
}
fn asset() -> ScreenAsset {
    ScreenAsset {
        id: "1839512626364936193".into(),
        source: "platform".into(),
        fields: fields(),
        app_version: Some("2.0.9".into()),
        platform_status: "online".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn confirmed_platform_deletion_removes_local_asset_but_keeps_task_history() {
    let dir=tempfile::tempdir().unwrap();let store=LocalStore::open(dir.path().join("db")).await.unwrap();
    project(&store,"a").await;let repo=ScreenRepository::new(store.pool().clone());
    repo.set_scope("a","business","source").await.unwrap();
    let local=repo.save_local("a",&fields(),None,None).await.unwrap();
    let platform=asset();let revision=repo.project_connection_revision("a").await.unwrap();
    repo.replace_platform_snapshot("a","business","source",&revision,&[platform.clone()],&[]).await.unwrap();
    sqlx::query("INSERT INTO local_screen_binding VALUES('a',?,'business',?,'request','{}',?)").bind(&local).bind(&platform.id).bind(now()).execute(store.pool()).await.unwrap();
    sqlx::query("INSERT INTO local_task(id,local_project_id,domain_type,operation_type,state,log_path,created_at,updated_at) VALUES('history','a','smart_screen','inspect','succeeded','fixture.jsonl',?,?)").bind(now()).bind(now()).execute(store.pool()).await.unwrap();
    let mut changed=platform.fields.clone();changed.name="草稿".into();
    repo.save_draft_checked("a",&platform.id,&changed,1,0).await.unwrap();
    // 仅目录中没出现，不能删除绑定；迟到的旧连接结果也不能删除。
    repo.replace_platform_snapshot("a","business","source",&revision,&[],&[]).await.unwrap();
    assert_eq!(repo.known_platform_ids("a","business").await.unwrap(),vec![platform.id.clone()]);
    assert!(repo.replace_snapshot_and_deleted("a","business","source","wrong",&[],&[],&[platform.id.clone()]).await.is_err());
    repo.replace_snapshot_and_deleted("a","business","source",&revision,&[],&[],&[platform.id]).await.unwrap();
    for table in ["local_screen","local_screen_binding","local_screen_draft"] {
        assert_eq!(sqlx::query_scalar::<_,i64>(&format!("SELECT COUNT(*) FROM {table}")).fetch_one(store.pool()).await.unwrap(),0);
    }
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM local_task WHERE id='history'").fetch_one(store.pool()).await.unwrap(),1);
    store.close().await;
}

#[tokio::test]
async fn empty_old_scope_can_be_reidentified_without_changing_aio_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::open(dir.path().join("db")).await.unwrap();
    project(&store, "a").await;
    let repo = ScreenRepository::new(store.pool().clone());
    sqlx::query("INSERT INTO local_task(id,local_project_id,domain_type,operation_type,state,log_path,created_at,updated_at) VALUES('aio-history','a','aio','first_deploy','succeeded','fixture.jsonl',?,?)").bind(now()).bind(now()).execute(store.pool()).await.unwrap();
    repo.set_scope("a", "business-a", "old-server:old-db")
        .await
        .unwrap();
    repo.clear_unused_scope("a", "business-a", "old-server:old-db")
        .await
        .unwrap();
    assert!(repo.scope("a").await.unwrap().is_none());
    let revision = repo.project_connection_revision("a").await.unwrap();
    repo.replace_platform_snapshot(
        "a",
        "business-b",
        "new-server:new-db",
        &revision,
        &[asset()],
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        repo.scope("a").await.unwrap(),
        Some(("business-b".into(), "new-server:new-db".into()))
    );
    // 迟到的旧刷新不能移除已经建立的新关联。
    repo.clear_unused_scope("a", "business-a", "old-server:old-db")
        .await
        .unwrap();
    assert_eq!(repo.snapshot("a").await.unwrap().screens.len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM local_task WHERE id='aio-history' AND state='succeeded'"
        )
        .fetch_one(store.pool())
        .await
        .unwrap(),
        1
    );
    store.close().await;
}

#[tokio::test]
async fn scope_and_cache_are_atomic_and_reject_reads_from_old_connection() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::open(dir.path().join("db")).await.unwrap();
    project(&store, "a").await;
    let repo = ScreenRepository::new(store.pool().clone());
    let revision = repo.project_connection_revision("a").await.unwrap();
    sqlx::query("CREATE TRIGGER fail_screen_cache BEFORE INSERT ON local_screen_platform_cache BEGIN SELECT RAISE(ABORT,'fixture cache failure'); END").execute(store.pool()).await.unwrap();
    assert!(
        repo.replace_platform_snapshot("a", "business", "source", &revision, &[asset()], &[])
            .await
            .is_err()
    );
    assert!(repo.scope("a").await.unwrap().is_none());
    sqlx::query("DROP TRIGGER fail_screen_cache")
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE local_project SET db_host='192.0.2.142' WHERE id='a'")
        .execute(store.pool())
        .await
        .unwrap();
    assert!(
        repo.replace_platform_snapshot("a", "business", "source", &revision, &[asset()], &[])
            .await
            .is_err()
    );
    assert!(repo.scope("a").await.unwrap().is_none());
    assert!(repo.snapshot("a").await.unwrap().screens.is_empty());
    store.close().await;
}

#[tokio::test]
async fn existing_screen_data_and_pending_tasks_prevent_scope_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::open(dir.path().join("db")).await.unwrap();
    let repo = ScreenRepository::new(store.pool().clone());
    for key in ["local", "cache", "history", "pending"] {
        project(&store, key).await;
        repo.set_scope(key, "business", "source").await.unwrap();
    }
    repo.save_local("local", &fields(), None, None)
        .await
        .unwrap();
    repo.replace_platform_cache("cache", "business", &[asset()], &[])
        .await
        .unwrap();
    sqlx::query("INSERT INTO local_task(id,local_project_id,domain_type,operation_type,state,log_path,created_at,updated_at) VALUES('screen-history','history','smart_screen','inspect','succeeded','fixture.jsonl',?,?),('aio-pending','pending','aio','first_deploy','running','fixture.jsonl',?,?)").bind(now()).bind(now()).bind(now()).bind(now()).execute(store.pool()).await.unwrap();
    for key in ["local", "cache", "history", "pending"] {
        assert!(
            repo.clear_unused_scope(key, "business", "source")
                .await
                .is_err()
        );
        assert_eq!(
            repo.scope(key).await.unwrap(),
            Some(("business".into(), "source".into()))
        );
    }
    assert_eq!(repo.snapshot("local").await.unwrap().screens.len(), 1);
    assert_eq!(repo.snapshot("cache").await.unwrap().screens.len(), 1);
    store.close().await;
}

#[tokio::test]
async fn local_assets_are_isolated_persistent_and_optimistic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("local.sqlite");
    let store = LocalStore::open(&path).await.unwrap();
    project(&store, "a").await;
    project(&store, "b").await;
    let repo = ScreenRepository::new(store.pool().clone());
    let id = repo.save_local("a", &fields(), None, None).await.unwrap();
    assert!(repo.snapshot("b").await.unwrap().screens.is_empty());
    assert!(
        repo.save_local("missing", &fields(), None, None)
            .await
            .is_err()
    );
    let mut next = fields();
    next.name = "修改后".into();
    repo.save_local("a", &next, Some(&id), Some(1))
        .await
        .unwrap();
    assert!(
        repo.save_local("a", &fields(), Some(&id), Some(1))
            .await
            .is_err()
    );
    assert!(
        repo.save_local("b", &fields(), Some(&id), Some(2))
            .await
            .is_err()
    );
    store.close().await;
    let reopened = LocalStore::open(&path).await.unwrap();
    let rows = ScreenRepository::new(reopened.pool().clone())
        .snapshot("a")
        .await
        .unwrap();
    assert_eq!(rows.screens.len(), 1);
    assert_eq!(rows.screens[0].id, id);
    assert_eq!(rows.screens[0].fields.name, "修改后");
    reopened.close().await;
}

#[tokio::test]
async fn imports_validate_all_rows_before_writing_and_keep_projects_separate() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::open(dir.path().join("db")).await.unwrap();
    project(&store, "a").await;
    let repo = ScreenRepository::new(store.pool().clone());
    let mut invalid = fields();
    invalid.ip = "127.0.0.1; erase".into();
    assert!(repo.import_local("a", &[fields(), invalid]).await.is_err());
    assert!(repo.snapshot("a").await.unwrap().screens.is_empty());
    let mut invalid_space = fields();
    invalid_space.space_id = Some("missing".into());
    assert!(repo.import_local("a", &[invalid_space]).await.is_err());
    let ids = repo.import_local("a", &[fields(), fields()]).await.unwrap();
    assert_ne!(ids[0], ids[1]);
    repo.remove_local("a", &ids[0]).await.unwrap();
    assert_eq!(repo.snapshot("a").await.unwrap().screens.len(), 1);
    store.close().await;
}

#[tokio::test]
async fn cache_scope_and_draft_do_not_overwrite_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::open(dir.path().join("db")).await.unwrap();
    project(&store, "a").await;
    let repo = ScreenRepository::new(store.pool().clone());
    repo.set_scope("a", "business-a", "source").await.unwrap();
    assert!(repo.set_scope("a", "business-b", "source").await.is_err());
    assert!(
        repo.replace_platform_cache("a", "business-b", &[asset()], &[])
            .await
            .is_err()
    );
    repo.replace_platform_cache("a", "business-a", &[asset()], &[])
        .await
        .unwrap();
    let id = asset().id;
    let mut draft = fields();
    draft.name = "待提交".into();
    repo.save_draft("a", &id, &draft, None).await.unwrap();
    assert!(repo.save_draft("a", &id, &fields(), None).await.is_err());
    let mut changed = asset();
    changed.fields.location = "平台新位置".into();
    repo.replace_platform_cache("a", "business-a", &[changed], &[])
        .await
        .unwrap();
    let snapshot = repo.snapshot("a").await.unwrap();
    assert_eq!(snapshot.platform_drafts[&id].base.location, "桌面");
    assert_eq!(snapshot.platform_drafts[&id].values.name, "待提交");
    assert_eq!(snapshot.screens[0].fields.location, "平台新位置");
    assert_eq!(snapshot.screens[0].app_version.as_deref(), Some("2.0.9"));
    assert!(!snapshot.platform_available);
    assert!(snapshot.spaces_available);
    repo.save_draft("a", &id, &draft, Some(1)).await.unwrap();
    assert!(repo.discard_draft("a", &id, 1).await.is_err());
    repo.discard_draft("a", &id, 2).await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn observations_are_immutable_and_repeated_save_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::open(dir.path().join("db")).await.unwrap();
    project(&store, "a").await;
    project(&store, "b").await;
    let repo = ScreenRepository::new(store.pool().clone());
    let id = repo.save_local("a", &fields(), None, None).await.unwrap();
    let item = ScreenObservation {
        id: uuid::Uuid::now_v7().to_string(),
        screen_id: id.clone(),
        observed_ip: fields().ip,
        observed_at: now(),
        observed_app_version: Some("2.0.9".into()),
        ..Default::default()
    };
    repo.append_observation("a", &item).await.unwrap();
    repo.append_observation("a", &item).await.unwrap();
    let mut changed = item.clone();
    changed.observed_app_version = Some("2.0.10".into());
    assert!(repo.append_observation("a", &changed).await.is_err());
    assert!(repo.append_observation("b", &item).await.is_err());
    let snapshot = repo.snapshot("a").await.unwrap();
    assert_eq!(snapshot.observations[&id].len(), 1);
    assert_eq!(snapshot.screens[0].app_version, None);
    store.close().await;
}

#[tokio::test]
async fn pending_write_preserves_request_identity_and_blocks_project_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::open(dir.path().join("db")).await.unwrap();
    project(&store, "a").await;
    let repo = ScreenRepository::new(store.pool().clone());
    repo.set_scope("a", "business", "source").await.unwrap();
    let id = repo.save_local("a", &fields(), None, None).await.unwrap();
    let intent = WriteIntent {
        request_id: uuid::Uuid::now_v7().to_string(),
        business_project_id: "business".into(),
        screen_id: id.clone(),
        platform_screen_id: "999".into(),
        operation_type: "register".into(),
        payload: serde_json::json!({"after":fields()}),
        state: "prepared".into(),
        result: None,
    };
    repo.prepare_intent("a", &intent).await.unwrap();
    repo.prepare_intent("a", &intent).await.unwrap();
    assert_eq!(repo.intents("a").await.unwrap().len(), 1);
    let mut changed = intent.clone();
    changed.platform_screen_id = "888".into();
    assert!(repo.prepare_intent("a", &changed).await.is_err());
    assert!(repo.remove_local("a", &id).await.is_err());
    assert!(
        sqlx::query("DELETE FROM local_project WHERE id='a'")
            .execute(store.pool())
            .await
            .is_err()
    );
    assert!(
        repo.update_intent("a", &intent.request_id, "prepared", "confirmed", None)
            .await
            .is_err()
    );
    repo.update_intent("a", &intent.request_id, "prepared", "submitted", None)
        .await
        .unwrap();
    assert!(
        repo.update_intent("a", &intent.request_id, "prepared", "submitted", None)
            .await
            .is_err()
    );
    repo.update_intent(
        "a",
        &intent.request_id,
        "submitted",
        "confirmed",
        Some(&serde_json::json!({"id":"999"})),
    )
    .await
    .unwrap();
    assert_eq!(
        repo.intents("a").await.unwrap()[0].platform_screen_id,
        "999"
    );
    store.close().await;
}

#[test]
fn device_business_and_shared_results_remain_distinct() {
    let result = ScreenTargetResult {
        format_version: 1,
        screen_id: "local".into(),
        device: ResultState::Succeeded,
        business: ResultState::Failed,
        shared: ResultState::Pending,
        ..Default::default()
    };
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["device"], "succeeded");
    assert_eq!(json["business"], "failed");
    assert_eq!(json["shared"], "pending");
    assert!(json.get("version").is_none());
}

#[tokio::test]
async fn confirmed_binding_is_atomic_idempotent_and_keeps_local_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::open(dir.path().join("db")).await.unwrap();
    project(&store, "a").await;
    let repo = ScreenRepository::new(store.pool().clone());
    repo.set_scope("a", "business", "source").await.unwrap();
    let id = repo.save_local("a", &fields(), None, None).await.unwrap();
    let observation = ScreenObservation {
        id: "observed-before-register".into(),
        screen_id: id.clone(),
        observed_ip: fields().ip,
        observed_at: now(),
        ..Default::default()
    };
    repo.append_observation("a", &observation).await.unwrap();
    let receipt = asset();
    let intent = WriteIntent {
        request_id: "request-1".into(),
        business_project_id: "business".into(),
        screen_id: id.clone(),
        platform_screen_id: receipt.id.clone(),
        operation_type: "register".into(),
        payload: serde_json::json!({"after":receipt.fields}),
        state: "prepared".into(),
        result: None,
    };
    repo.prepare_intent("a", &intent).await.unwrap();
    repo.update_intent("a", "request-1", "prepared", "submitted", None)
        .await
        .unwrap();
    repo.confirm_registration(
        "a",
        "business",
        &id,
        true,
        "request-1",
        &receipt,
        &receipt.fields,
        0,
        &serde_json::json!({"after":receipt}),
    )
    .await
    .unwrap();
    let mut old_replay = receipt.clone();
    old_replay.fields.name = "不能覆盖的新值".into();
    repo.confirm_registration(
        "a",
        "business",
        &id,
        true,
        "request-1",
        &old_replay,
        &old_replay.fields,
        0,
        &serde_json::Value::Null,
    )
    .await
    .unwrap();
    let snapshot = repo.snapshot("a").await.unwrap();
    assert_eq!(snapshot.screens.len(), 1);
    assert_eq!(snapshot.screens[0].source, "platform");
    assert_eq!(snapshot.screens[0].fields.name, receipt.fields.name);
    assert_eq!(snapshot.screens[0].aliases, vec![id.clone()]);
    assert_eq!(snapshot.observations[&id].len(), 1);
    sqlx::query("DELETE FROM local_project WHERE id='a'")
        .execute(store.pool())
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_binding")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    store.close().await;
}

#[tokio::test]
async fn new_draft_can_explicitly_undo_value_while_previous_submission_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalStore::open(dir.path().join("db")).await.unwrap();
    project(&store, "a").await;
    let repo = ScreenRepository::new(store.pool().clone());
    repo.set_scope("a", "business", "source").await.unwrap();
    let original = asset();
    repo.replace_platform_cache("a", "business", &[original.clone()], &[])
        .await
        .unwrap();
    let mut submitted = original.fields.clone();
    submitted.name = "第一次提交".into();
    repo.save_draft_checked("a", &original.id, &submitted, 1, 0)
        .await
        .unwrap();
    repo.save_draft_checked("a", &original.id, &original.fields, 1, 1)
        .await
        .unwrap();
    let intent = WriteIntent {
        request_id: "update-request".into(),
        business_project_id: "business".into(),
        screen_id: original.id.clone(),
        platform_screen_id: original.id.clone(),
        operation_type: "update".into(),
        payload: serde_json::json!({"after":submitted}),
        state: "prepared".into(),
        result: None,
    };
    repo.prepare_intent("a", &intent).await.unwrap();
    repo.update_intent("a", &intent.request_id, "prepared", "submitted", None)
        .await
        .unwrap();
    let mut saved = original.clone();
    saved.fields = submitted.clone();
    repo.confirm_registration(
        "a",
        "business",
        &original.id,
        false,
        &intent.request_id,
        &saved,
        &submitted,
        1,
        &serde_json::json!({"after":saved}),
    )
    .await
    .unwrap();
    let snapshot = repo.snapshot("a").await.unwrap();
    let draft = &snapshot.platform_drafts[&original.id];
    assert_eq!(draft.base.name, "第一次提交");
    assert_eq!(draft.values.name, original.fields.name);
    store.close().await;
}
