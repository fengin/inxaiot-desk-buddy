use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use inxaiot_desk_buddy_lib::core::secret::SecretValue;
use inxaiot_desk_buddy_lib::formal::error::FormalError;
use inxaiot_desk_buddy_lib::formal::operation_repository::{
    OperationRepository, OperationStart, TargetFinalResult,
};
use inxaiot_desk_buddy_lib::formal::resource_lease_repository::{
    LeaseGrant, LeaseRequest, ResourceLeaseRepository,
};
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use inxaiot_desk_buddy_lib::infrastructure::database::{
    DatabaseTlsMode, DualMySqlPools, MySqlProjectConfig,
};
use inxaiot_desk_buddy_lib::infrastructure::deployment_control::start_deployment_heartbeat_with_timing;
use tokio::sync::Barrier;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn line<'a>(text: &'a str, label: &str) -> &'a str {
    text.lines()
        .find_map(|item| item.trim().strip_prefix(label))
        .map(str::trim)
        .unwrap_or_else(|| panic!("missing {label}"))
}

fn default<'a>(text: &'a str, key: &str) -> &'a str {
    let marker = "$".to_string() + "{" + key + ":";
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

fn lease_request(
    resource_key: &str,
    operation_id: &str,
    owner: &str,
    ttl: Duration,
) -> LeaseRequest {
    LeaseRequest {
        resource_type: "aio".into(),
        resource_key: resource_key.into(),
        domain_type: "aio".into(),
        operation_id: operation_id.into(),
        owner_instance_id: owner.into(),
        owner_user: "phase7-recovery-test".into(),
        ttl,
    }
}

async fn start_operation(
    operations: &OperationRepository,
    targets: &[String],
) -> inxaiot_desk_buddy_lib::formal::operation_repository::OperationRecord {
    operations
        .start(OperationStart {
            domain_type: "aio".into(),
            operation_type: "full_upgrade".into(),
            operation_name: "phase7 recovery gate".into(),
            operator_name: "phase7-recovery-test".into(),
            instance_id: "phase7-instance-a".into(),
            targets: targets
                .iter()
                .map(|mac| ("aio".into(), mac.clone()))
                .collect(),
            artifact_name: Some("Release".into()),
            artifact_version: Some("phase7-recovery".into()),
            operation_summary: Some(serde_json::json!({"gate": "stage7-recovery"})),
            retry_of_operation_id: None,
        })
        .await
        .expect("start phase7 operation")
}

#[tokio::test]
#[ignore = "writes one isolated operation, waits for exact leases to expire, then precisely cleans it"]
async fn stale_operation_keeps_known_result_and_marks_pending_target_unknown() {
    let pools = DualMySqlPools::connect(&config())
        .await
        .expect("connect project mysql");
    WorkbenchStore::new(pools.workbench.clone())
        .migrate()
        .await
        .expect("workbench schema");
    let operations = OperationRepository::new(pools.workbench.clone());
    let leases = ResourceLeaseRepository::new(pools.workbench.clone());
    let targets = vec![random_mac(), random_mac()];
    let started = start_operation(&operations, &targets).await;
    let operation_id = started.id.clone();
    let worker_operations = operations.clone();
    let worker_leases = leases.clone();
    let worker_targets = targets.clone();
    let worker = tokio::spawn(async move {
        worker_operations
            .finalize_target(TargetFinalResult {
                operation_id: started.id.clone(),
                resource_type: "aio".into(),
                resource_key: worker_targets[0].clone(),
                result_state: "succeeded".into(),
                before_version: None,
                after_version: Some("phase7-recovery".into()),
                result_summary: Some("known remote result".into()),
                error_code: None,
                error_summary: None,
            })
            .await
            .expect("persist known target result");
        worker_leases
            .acquire_many(
                worker_targets
                    .iter()
                    .map(|mac| {
                        lease_request(
                            mac,
                            &started.id,
                            "phase7-instance-a",
                            Duration::from_secs(1),
                        )
                    })
                    .collect(),
            )
            .await
            .expect("acquire short leases");
        tokio::time::sleep(Duration::from_millis(2200)).await;
        let candidates = worker_operations
            .list_stale_candidates(Duration::from_secs(1), 20)
            .await
            .expect("list stale operations");
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.id == started.id)
        );
        let recovered = worker_operations
            .interrupt_stale(&started.id, started.version, Duration::from_secs(1))
            .await
            .expect("interrupt stale operation");
        assert_eq!(recovered.state, "interrupted");
        assert_eq!(recovered.success_count, 1);
        assert_eq!(recovered.failure_count, 1);
        assert_eq!(recovered.version, started.version + 1);
    })
    .await;
    operations
        .delete_test_operation(&operation_id)
        .await
        .expect("precisely clean stage7 recovery operation");
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
    .expect("verify stage7 recovery cleanup");
    assert_eq!(remaining, 0);
    pools.close().await;
    worker.expect("stage7 recovery worker");
    println!("stage7 recovery operation={operation_id} remaining=0");
}

#[tokio::test]
#[ignore = "runs two isolated owners against one random lease key and removes only that counter"]
async fn concurrent_instances_allow_exactly_one_lease_owner() {
    let pools = DualMySqlPools::connect(&config())
        .await
        .expect("connect project mysql");
    let repository = ResourceLeaseRepository::new(pools.workbench.clone());
    let resource_key = format!("phase7:lease:{}", Uuid::now_v7());
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for owner in ["phase7-instance-a", "phase7-instance-b"] {
        let barrier = barrier.clone();
        let repository = repository.clone();
        let resource_key = resource_key.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            repository
                .acquire_many(vec![lease_request(
                    &resource_key,
                    &Uuid::now_v7().to_string(),
                    owner,
                    Duration::from_secs(30),
                )])
                .await
        }));
    }
    barrier.wait().await;
    let mut winner: Option<LeaseGrant> = None;
    let mut conflicts = 0;
    for handle in handles {
        match handle.await.expect("lease contender") {
            Ok(mut grants) => winner = grants.pop(),
            Err(FormalError::Conflict(_)) => conflicts += 1,
            Err(error) => panic!("unexpected lease error: {error}"),
        }
    }
    assert_eq!(conflicts, 1);
    let winner = winner.expect("one lease winner");
    repository.release(&winner).await.expect("release winner");
    repository
        .delete_test_counter("aio", &resource_key)
        .await
        .expect("clean exact stage7 counter");
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM resource_lease WHERE resource_type = 'aio' AND resource_key = ?",
    )
    .bind(&resource_key)
    .fetch_one(&pools.workbench)
    .await
    .expect("verify lease cleanup");
    assert_eq!(remaining, 0);
    pools.close().await;
    println!("stage7 concurrent lease key={resource_key} remaining=0");
}

#[tokio::test]
#[ignore = "closes one isolated workbench pool to verify heartbeat cancellation and exact cleanup"]
async fn database_disconnect_cancels_execution_before_more_remote_steps() {
    let config = config();
    let pools = DualMySqlPools::connect(&config)
        .await
        .expect("connect project mysql");
    let operations = OperationRepository::new(pools.workbench.clone());
    let leases = ResourceLeaseRepository::new(pools.workbench.clone());
    let target = random_mac();
    let started = start_operation(&operations, std::slice::from_ref(&target)).await;
    let operation_id = started.id.clone();
    let grants = leases
        .acquire_many(vec![lease_request(
            &target,
            &started.id,
            "phase7-instance-a",
            Duration::from_secs(2),
        )])
        .await
        .expect("acquire heartbeat lease");
    let execution_cancellation = CancellationToken::new();
    let heartbeat = start_deployment_heartbeat_with_timing(
        pools.workbench.clone(),
        started.id,
        started.version,
        grants,
        execution_cancellation.clone(),
        Duration::from_millis(80),
        Duration::from_secs(2),
    );
    tokio::time::sleep(Duration::from_millis(140)).await;
    pools.workbench.close().await;
    tokio::time::timeout(Duration::from_secs(3), execution_cancellation.cancelled())
        .await
        .expect("heartbeat should cancel execution after pool disconnect");
    assert!(heartbeat.stop().await.is_err());
    pools.close().await;

    let cleanup_pools = DualMySqlPools::connect(&config)
        .await
        .expect("reconnect project mysql");
    OperationRepository::new(cleanup_pools.workbench.clone())
        .delete_test_operation(&operation_id)
        .await
        .expect("precisely clean disconnect operation");
    let remaining: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM operation_record WHERE id = ?) + \
         (SELECT COUNT(*) FROM operation_target_result WHERE operation_id = ?) + \
         (SELECT COUNT(*) FROM resource_lease WHERE operation_id = ?)",
    )
    .bind(&operation_id)
    .bind(&operation_id)
    .bind(&operation_id)
    .fetch_one(&cleanup_pools.workbench)
    .await
    .expect("verify disconnect cleanup");
    assert_eq!(remaining, 0);
    cleanup_pools.close().await;
    println!("stage7 disconnect operation={operation_id} remaining=0");
}
