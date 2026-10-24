use std::time::Duration;

use inxaiot_desk_buddy_lib::{
    core::error::AppError,
    domain::smart_screen::model::{ScreenAsset, ScreenFields, WriteIntent},
    formal::local_store::LocalStore,
    infrastructure::local_sqlite::screen_repository::{ScreenRepository, now},
};
use serde_json::{Value, json};
use sqlx::Row;

struct Fixture {
    _directory: tempfile::TempDir,
    store: LocalStore,
    repository: ScreenRepository,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = LocalStore::open(directory.path().join("confirmation.db"))
            .await
            .unwrap();
        for project in ["project", "other-project"] {
            sqlx::query("INSERT INTO local_project(id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) VALUES(?,?,'','',3306,'','','inxaiot_desk_buddy','',?,?)")
                .bind(project).bind(project).bind(now()).bind(now()).execute(store.pool()).await.unwrap();
        }
        let repository = ScreenRepository::new(store.pool().clone());
        repository
            .set_scope("project", "business", "synthetic-source")
            .await
            .unwrap();
        Self {
            _directory: directory,
            store,
            repository,
        }
    }

    async fn submitted(&self, index: usize, operation: &str) -> (String, ScreenAsset, Value) {
        let mut asset = ScreenAsset {
            id: format!("screen-{index}"),
            source: "platform".into(),
            fields: ScreenFields {
                name: format!("合成测试屏{index}"),
                ip: format!("192.0.2.{}", index + 1),
                size: "10".into(),
                ..Default::default()
            },
            app_version: Some("2.1.2".into()),
            platform_status: "offline".into(),
            ..Default::default()
        };
        let request = format!("request-{index}");
        // 只准备合成缓存和本机意图，不使用平台连接或设备命令。
        sqlx::query("INSERT INTO local_screen_platform_cache(local_project_id,business_project_id,platform_screen_id,asset_json,read_batch,read_at) VALUES('project','business',?,?,?,?)")
            .bind(&asset.id).bind(serde_json::to_string(&asset).unwrap()).bind("initial").bind(now())
            .execute(self.store.pool()).await.unwrap();
        self.repository
            .prepare_intent("project", &WriteIntent {
                request_id: request.clone(),
                business_project_id: "business".into(),
                screen_id: asset.id.clone(),
                platform_screen_id: asset.id.clone(),
                operation_type: operation.into(),
                payload: json!({"field": if operation == "status" { "online" } else { "app_version" }, "before":"old", "after":"new"}),
                state: "prepared".into(),
                result: None,
            })
            .await
            .unwrap();
        self.repository
            .update_intent("project", &request, "prepared", "submitted", None)
            .await
            .unwrap();
        asset.app_version = Some("2.1.3".into());
        asset.platform_status = "online".into();
        let evidence = json!({"after": asset, "verified": true});
        (request, asset, evidence)
    }

    async fn cache(&self, screen: &str) -> (Value, i64) {
        let row = sqlx::query("SELECT asset_json,revision FROM local_screen_platform_cache WHERE local_project_id='project' AND business_project_id='business' AND platform_screen_id=?")
            .bind(screen).fetch_one(self.store.pool()).await.unwrap();
        (
            serde_json::from_str(row.get::<&str, _>("asset_json")).unwrap(),
            row.get("revision"),
        )
    }

    async fn intent(&self, request: &str) -> (String, Option<String>) {
        let row = sqlx::query(
            "SELECT state,result_json FROM local_screen_write_intent WHERE request_id=?",
        )
        .bind(request)
        .fetch_one(self.store.pool())
        .await
        .unwrap();
        (row.get("state"), row.get("result_json"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_confirmations_wait_before_reading_and_all_retain_their_receipts() {
    let fixture = Fixture::new().await;
    let mut submitted = Vec::new();
    for index in 0..20 {
        submitted.push(fixture.submitted(index, "install_version").await);
    }
    // 独立连接持有写事务；旧 BEGIN→SELECT→写入结构在此会立即读升写失败。
    let mut writer = fixture
        .store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .unwrap();
    sqlx::query("UPDATE local_project SET updated_at=? WHERE id='project'")
        .bind(now())
        .execute(&mut *writer)
        .await
        .unwrap();
    let gate = std::sync::Arc::new(tokio::sync::Barrier::new(submitted.len() + 1));
    let mut pending = Vec::new();
    for (request, asset, evidence) in &submitted {
        let (request, asset, evidence) = (request.clone(), asset.clone(), evidence.clone());
        let repository = fixture.repository.clone();
        let gate = gate.clone();
        pending.push(tokio::spawn(async move {
            gate.wait().await;
            repository
                .confirm_business_value("project", "business", &request, &asset, &evidence)
                .await
        }));
    }
    gate.wait().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        pending.iter().all(|task| !task.is_finished()),
        "本机确认应等待写锁，不能读取旧快照后立即失败"
    );
    writer.commit().await.unwrap();
    for task in pending {
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    for (request, asset, evidence) in submitted {
        let (cache, _) = fixture.cache(&asset.id).await;
        assert_eq!(cache["appVersion"], "2.1.3");
        let (state, proof) = fixture.intent(&request).await;
        assert_eq!(state, "confirmed");
        assert_eq!(
            serde_json::from_str::<Value>(&proof.unwrap()).unwrap(),
            evidence
        );
    }
    fixture.store.close().await;
}

#[tokio::test]
async fn failed_confirmation_rolls_back_cache_and_intent_then_can_be_confirmed_locally() {
    let fixture = Fixture::new().await;
    let (request, asset, evidence) = fixture.submitted(0, "install_version").await;
    let before = fixture.cache(&asset.id).await;
    sqlx::query("CREATE TRIGGER fail_confirmation BEFORE UPDATE OF state ON local_screen_write_intent WHEN NEW.state='confirmed' BEGIN SELECT RAISE(ABORT,'synthetic confirmation write rejected'); END")
        .execute(fixture.store.pool()).await.unwrap();
    let error = fixture
        .repository
        .confirm_business_value("project", "business", &request, &asset, &evidence)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Database { .. }));
    assert_eq!(fixture.cache(&asset.id).await, before);
    assert_eq!(fixture.intent(&request).await, ("submitted".into(), None));
    sqlx::query("DROP TRIGGER fail_confirmation")
        .execute(fixture.store.pool())
        .await
        .unwrap();
    fixture
        .repository
        .confirm_business_value("project", "business", &request, &asset, &evidence)
        .await
        .unwrap();
    assert_eq!(fixture.cache(&asset.id).await.0["appVersion"], "2.1.3");
    assert_eq!(fixture.intent(&request).await.0, "confirmed");
    fixture.store.close().await;
}

#[tokio::test]
async fn busy_confirmation_exhausts_existing_wait_without_falsely_confirming_or_clearing_intent() {
    let fixture = Fixture::new().await;
    let (request, asset, evidence) = fixture.submitted(0, "install_version").await;
    let before = fixture.cache(&asset.id).await;
    let mut writer = fixture
        .store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .unwrap();
    sqlx::query("UPDATE local_project SET updated_at=? WHERE id='project'")
        .bind(now())
        .execute(&mut *writer)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let error = tokio::time::timeout(
        Duration::from_secs(10),
        fixture
            .repository
            .confirm_business_value("project", "business", &request, &asset, &evidence),
    )
    .await
    .expect("本机数据库确认仍须有界返回")
    .unwrap_err();
    assert!(matches!(error, AppError::Database { .. }));
    assert!(
        started.elapsed() >= Duration::from_secs(4),
        "应沿用已有5秒busy等待，而非立即报读快照冲突"
    );
    // WAL允许其他连接继续只读；未成功取得写锁不能误清请求或提前刷新缓存。
    assert_eq!(fixture.intent(&request).await, ("submitted".into(), None));
    assert_eq!(fixture.cache(&asset.id).await, before);
    writer.commit().await.unwrap();
    fixture
        .repository
        .confirm_business_value("project", "business", &request, &asset, &evidence)
        .await
        .unwrap();
    assert_eq!(fixture.intent(&request).await.0, "confirmed");
    assert_eq!(fixture.cache(&asset.id).await.0["appVersion"], "2.1.3");
    fixture.store.close().await;
}

#[tokio::test]
async fn confirmed_requests_are_idempotent_for_install_version_and_status_updates() {
    let fixture = Fixture::new().await;
    for (index, operation) in ["install_version", "version_sync", "status"]
        .into_iter()
        .enumerate()
    {
        let (request, asset, evidence) = fixture.submitted(index, operation).await;
        fixture
            .repository
            .confirm_business_value("project", "business", &request, &asset, &evidence)
            .await
            .unwrap();
        let confirmed_cache = fixture.cache(&asset.id).await;
        let confirmed_intent = fixture.intent(&request).await;
        let mut stale = asset.clone();
        stale.app_version = Some("1.0.0".into());
        stale.platform_status = "offline".into();
        fixture
            .repository
            .confirm_business_value(
                "project",
                "business",
                &request,
                &stale,
                &json!({"stale":true}),
            )
            .await
            .unwrap();
        assert_eq!(fixture.cache(&asset.id).await, confirmed_cache);
        assert_eq!(fixture.intent(&request).await, confirmed_intent);
    }
    fixture.store.close().await;
}

#[tokio::test]
async fn confirmation_rejects_wrong_scope_target_request_and_unsubmitted_states_atomically() {
    let fixture = Fixture::new().await;
    let (request, asset, evidence) = fixture.submitted(0, "install_version").await;
    let before = fixture.cache(&asset.id).await;
    let mut wrong = asset.clone();
    wrong.id = "another-screen".into();
    for (project, business, request, target) in [
        ("other-project", "business", request.as_str(), &asset),
        ("project", "other-business", request.as_str(), &asset),
        ("project", "business", "missing-request", &asset),
        ("project", "business", request.as_str(), &wrong),
    ] {
        assert!(matches!(
            fixture
                .repository
                .confirm_business_value(project, business, request, target, &evidence)
                .await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(fixture.cache(&asset.id).await, before);
    }
    for status in ["prepared", "not_applied", "conflict"] {
        sqlx::query("UPDATE local_screen_write_intent SET state=? WHERE request_id=?")
            .bind(status)
            .bind(&request)
            .execute(fixture.store.pool())
            .await
            .unwrap();
        assert!(matches!(
            fixture
                .repository
                .confirm_business_value("project", "business", &request, &asset, &evidence)
                .await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(fixture.intent(&request).await, (status.into(), None));
        assert_eq!(fixture.cache(&asset.id).await, before);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM local_screen_platform_cache")
            .fetch_one(fixture.store.pool())
            .await
            .unwrap(),
        1
    );
    fixture.store.close().await;
}
