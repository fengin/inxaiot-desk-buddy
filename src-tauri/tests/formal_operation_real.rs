use std::path::Path;
use std::time::Duration;

use inxaiot_desk_buddy_lib::core::secret::SecretValue;
use inxaiot_desk_buddy_lib::formal::error::FormalError;
use inxaiot_desk_buddy_lib::formal::operation_repository::{
    OperationFinalResult, OperationRepository, OperationStart, TargetFinalResult,
};
use inxaiot_desk_buddy_lib::formal::resource_lease_repository::{
    LeaseRequest, ResourceLeaseRepository,
};
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use inxaiot_desk_buddy_lib::infrastructure::database::{
    DatabaseTlsMode, DualMySqlPools, MySqlProjectConfig,
};
use sqlx::Row;
use uuid::Uuid;

fn line<'a>(text: &'a str, label: &str) -> &'a str {
    text.lines()
        .find_map(|item| item.trim().strip_prefix(label))
        .map(str::trim)
        .unwrap_or_else(|| panic!("missing {label}"))
}

fn default<'a>(text: &'a str, key: &str) -> &'a str {
    let marker = format!("${{{key}:");
    let rest = &text[text.find(&marker).expect("config default") + marker.len()..];
    &rest[..rest.find('}').expect("config default end")]
}

fn config() -> MySqlProjectConfig {
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project");
    let description =
        std::fs::read_to_string(project.join("test/测试数据说明.txt")).expect("test data");
    let workspace = project.parent().and_then(Path::parent).expect("workspace");
    let yaml = std::fs::read_to_string(workspace.join(
        "inxvision-platform/inxaiot-starter-platform/src/main/resources/application-dev.yml",
    ))
    .expect("platform config");
    MySqlProjectConfig {
        host: default(&yaml, "MYSQL_HOST").into(),
        port: default(&yaml, "MYSQL_PORT").parse().expect("mysql port"),
        username: default(&yaml, "MYSQL_USER").into(),
        password: SecretValue::new(default(&yaml, "MYSQL_PASSWORD")),
        platform_schema: line(&description, "平台业务数据库名：").into(),
        workbench_schema: line(&description, "工作台数据库：").into(),
        tls_mode: DatabaseTlsMode::Disabled,
        connect_timeout: Duration::from_secs(10),
    }
}

fn random_mac() -> String {
    let value = Uuid::now_v7().simple().to_string();
    value[value.len() - 12..].to_uppercase()
}

fn lease_request(resource_key: &str, operation_id: &str) -> LeaseRequest {
    LeaseRequest {
        resource_type: "aio".into(),
        resource_key: resource_key.into(),
        domain_type: "aio".into(),
        operation_id: operation_id.into(),
        owner_instance_id: "instance-operation-test".into(),
        owner_user: "phase3-operation-test".into(),
        ttl: Duration::from_secs(30),
    }
}

#[tokio::test]
#[ignore = "writes and precisely removes one isolated operation, two target results, and leases"]
async fn operation_targets_finalization_counts_version_and_exact_cleanup() {
    let config = config();
    let pools = DualMySqlPools::connect(&config)
        .await
        .expect("connect project mysql");
    WorkbenchStore::new(pools.workbench.clone())
        .migrate()
        .await
        .expect("workbench migration");
    let operations = OperationRepository::new(pools.workbench.clone());
    let leases = ResourceLeaseRepository::new(pools.workbench.clone());
    let first_mac = random_mac();
    let second_mac = random_mac();
    let started = operations
        .start(OperationStart {
            domain_type: "aio".into(),
            operation_type: "full_upgrade".into(),
            operation_name: "phase3 isolated operation".into(),
            operator_name: "phase3-operation-test".into(),
            instance_id: "instance-operation-test".into(),
            targets: vec![
                ("aio".into(), first_mac.clone()),
                ("aio".into(), second_mac.clone()),
            ],
            artifact_name: Some("Release".into()),
            artifact_version: Some("poc-version".into()),
            operation_summary: Some(serde_json::json!({
                "releaseMode": "full_upgrade",
                "releaseProfileVersion": 1
            })),
            retry_of_operation_id: None,
        })
        .await
        .expect("start operation");
    let operation_id = started.id.clone();
    let worker_operations = operations.clone();
    let worker_leases = leases.clone();
    let worker = tokio::spawn(async move {
        assert_eq!(started.state, "running");
        assert_eq!(started.target_count, 2);
        assert_eq!(started.version, 1);

        let mut grants = worker_leases
            .acquire_many(vec![
                lease_request(&first_mac, &started.id),
                lease_request(&second_mac, &started.id),
            ])
            .await
            .expect("acquire operation leases");
        assert_eq!(grants.len(), 2);

        assert!(worker_operations.heartbeat(&started.id, 0).await.is_err());
        let version = worker_operations
            .heartbeat(&started.id, 1)
            .await
            .expect("operation heartbeat");
        assert_eq!(version, 2);

        worker_operations
            .finalize_target(TargetFinalResult {
                operation_id: started.id.clone(),
                resource_type: "aio".into(),
                resource_key: first_mac.clone(),
                result_state: "succeeded".into(),
                before_version: Some("old".into()),
                after_version: Some("new".into()),
                result_summary: Some("service healthy".into()),
                error_code: None,
                error_summary: None,
            })
            .await
            .expect("finalize first target");
        let duplicate = worker_operations
            .finalize_target(TargetFinalResult {
                operation_id: started.id.clone(),
                resource_type: "aio".into(),
                resource_key: first_mac.clone(),
                result_state: "failed".into(),
                before_version: None,
                after_version: None,
                result_summary: None,
                error_code: Some("DUPLICATE".into()),
                error_summary: Some("must be rejected".into()),
            })
            .await;
        assert!(matches!(duplicate, Err(FormalError::Conflict(_))));
        let premature = worker_operations
            .finalize(OperationFinalResult {
                operation_id: started.id.clone(),
                expected_version: version,
                state: "partially_succeeded".into(),
                result_summary: None,
                error_code: None,
                error_summary: None,
            })
            .await;
        assert!(
            matches!(premature, Err(FormalError::Conflict(_))),
            "unexpected premature finalization result: {premature:?}"
        );

        worker_operations
            .finalize_target(TargetFinalResult {
                operation_id: started.id.clone(),
                resource_type: "aio".into(),
                resource_key: second_mac.clone(),
                result_state: "failed".into(),
                before_version: Some("old".into()),
                after_version: None,
                result_summary: Some("service check failed".into()),
                error_code: Some("SERVICE_CHECK_FAILED".into()),
                error_summary: Some("isolated failure".into()),
            })
            .await
            .expect("finalize second target");
        let stale_finalize = worker_operations
            .finalize(OperationFinalResult {
                operation_id: started.id.clone(),
                expected_version: 1,
                state: "partially_succeeded".into(),
                result_summary: None,
                error_code: None,
                error_summary: None,
            })
            .await;
        assert!(matches!(stale_finalize, Err(FormalError::Conflict(_))));
        let final_record = worker_operations
            .finalize(OperationFinalResult {
                operation_id: started.id.clone(),
                expected_version: version,
                state: "partially_succeeded".into(),
                result_summary: Some("one succeeded, one failed".into()),
                error_code: None,
                error_summary: None,
            })
            .await
            .expect("finalize operation");
        assert_eq!(final_record.success_count, 1);
        assert_eq!(final_record.failure_count, 1);
        assert_eq!(final_record.cancelled_count, 0);
        assert_eq!(final_record.version, 3);

        while let Some(grant) = grants.pop() {
            worker_leases
                .release(&grant)
                .await
                .expect("release operation lease");
        }
    });
    let worker_result = worker.await;
    operations
        .delete_test_operation(&operation_id)
        .await
        .expect("delete exact operation and leases");
    let remaining: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM operation_record WHERE id = ?) + \
         (SELECT COUNT(*) FROM operation_target_result WHERE operation_id = ?) + \
         (SELECT COUNT(*) FROM resource_lease WHERE operation_id = ?)",
    )
    .bind(&operation_id)
    .bind(&operation_id)
    .bind(&operation_id)
    .fetch_one(&pools.workbench)
    .await
    .expect("verify exact operation cleanup");
    assert_eq!(remaining, 0);
    println!("verified operation_id={operation_id} targets=2 remaining=0");
    pools.close().await;
    worker_result.expect("operation gate worker panicked");
}

#[tokio::test]
#[ignore = "read-only inspection for an interrupted isolated operation test"]
async fn inspect_interrupted_operation_candidates() {
    let config = config();
    let pools = DualMySqlPools::connect(&config)
        .await
        .expect("connect project mysql");
    let rows = sqlx::query(
        "SELECT id, state, target_count FROM operation_record \
         WHERE operation_name = ? AND operator_name = ? AND instance_id = ? \
         ORDER BY started_at DESC LIMIT 20",
    )
    .bind("phase3 isolated operation")
    .bind("phase3-operation-test")
    .bind("instance-operation-test")
    .fetch_all(&pools.workbench)
    .await
    .expect("read isolated operation candidates");
    for row in rows {
        let id: String = row.try_get("id").expect("operation id");
        let state: String = row.try_get("state").expect("operation state");
        let target_count: u32 = row.try_get("target_count").expect("target count");
        let persisted_targets: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM operation_target_result WHERE operation_id = ?",
        )
        .bind(&id)
        .fetch_one(&pools.workbench)
        .await
        .expect("target result count");
        let persisted_leases: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM resource_lease WHERE operation_id = ?")
                .bind(&id)
                .fetch_one(&pools.workbench)
                .await
                .expect("lease count");
        println!(
            "candidate id={id} state={state} target_count={target_count} persisted_targets={persisted_targets} persisted_leases={persisted_leases}"
        );
    }
    pools.close().await;
}

#[tokio::test]
#[ignore = "requires INX_TEST_OPERATION_ID and removes only that isolated operation"]
async fn cleanup_exact_interrupted_operation() {
    let operation_id = std::env::var("INX_TEST_OPERATION_ID").expect("exact operation id");
    let config = config();
    let pools = DualMySqlPools::connect(&config)
        .await
        .expect("connect project mysql");
    let marker_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM operation_record WHERE id = ? AND operation_name = ? \
         AND operator_name = ? AND instance_id = ?",
    )
    .bind(&operation_id)
    .bind("phase3 isolated operation")
    .bind("phase3-operation-test")
    .bind("instance-operation-test")
    .fetch_one(&pools.workbench)
    .await
    .expect("verify isolated operation marker");
    assert_eq!(marker_count, 1, "refuse to clean an unmarked operation");
    OperationRepository::new(pools.workbench.clone())
        .delete_test_operation(&operation_id)
        .await
        .expect("delete exact interrupted operation");
    let remaining: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM operation_record WHERE id = ?) + \
         (SELECT COUNT(*) FROM operation_target_result WHERE operation_id = ?) + \
         (SELECT COUNT(*) FROM resource_lease WHERE operation_id = ?)",
    )
    .bind(&operation_id)
    .bind(&operation_id)
    .bind(&operation_id)
    .fetch_one(&pools.workbench)
    .await
    .expect("verify exact interrupted operation cleanup");
    assert_eq!(remaining, 0);
    pools.close().await;
}
