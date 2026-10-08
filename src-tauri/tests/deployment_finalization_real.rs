#[path = "common/project_test_config.rs"]
mod project_test_config;

use std::time::Duration;

use inxaiot_desk_buddy_lib::formal::aio_node_repository::ServiceVersionWrite;
use inxaiot_desk_buddy_lib::formal::operation_repository::{
    OperationFinalResult, OperationRepository, OperationStart, TargetFinalResult,
};
use inxaiot_desk_buddy_lib::formal::resource_lease_repository::{
    LeaseRequest, ResourceLeaseRepository,
};
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use inxaiot_desk_buddy_lib::infrastructure::deployment_finalization::{
    AtomicDeploymentFinalization, AtomicTargetFinalization, finalize_deployment_atomically,
    shared_operation_state,
};
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode};
use sqlx::{Executor, MySqlPool, Row};
use uuid::Uuid;

struct TestConfig {
    host: String,
    port: u16,
    username: String,
    password: String,
    schema: String,
}

fn config() -> TestConfig {
    let database = project_test_config::database();
    let suffix = Uuid::now_v7().simple().to_string();
    TestConfig {
        host: database.host,
        port: database.port,
        username: database.username,
        password: database.password,
        schema: format!("inxaiot_desk_buddy_finalization_{}", &suffix[..12]),
    }
}

async fn pool(config: &TestConfig, database: Option<&str>) -> MySqlPool {
    let mut options = MySqlConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .username(&config.username)
        .password(&config.password)
        .ssl_mode(MySqlSslMode::Disabled);
    if let Some(database) = database {
        options = options.database(database);
    }
    MySqlPoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
        .expect("connect isolated mysql")
}

async fn create_schema(admin: &MySqlPool, schema: &str) {
    assert!(schema.starts_with("inxaiot_desk_buddy_finalization_"));
    assert!(
        schema
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    );
    admin
        .execute(
            format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci")
                .as_str(),
        )
        .await
        .expect("create finalization schema");
}

async fn drop_schema(admin: &MySqlPool, schema: &str) {
    assert!(schema.starts_with("inxaiot_desk_buddy_finalization_"));
    admin
        .execute(format!("DROP DATABASE IF EXISTS `{schema}`").as_str())
        .await
        .expect("drop finalization schema");
}

fn ensure(condition: bool, message: &'static str) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

#[tokio::test]
#[ignore = "read-only information_schema inspection for isolated finalization schemas"]
async fn inspect_finalization_schema_residue_read_only() {
    let config = config();
    let admin = pool(&config, None).await;
    let schemas = sqlx::query_scalar::<_, String>(
        "SELECT schema_name FROM information_schema.schemata \
         WHERE LEFT(schema_name, LENGTH('inxaiot_desk_buddy_finalization_')) = \
               'inxaiot_desk_buddy_finalization_' \
         ORDER BY schema_name",
    )
    .fetch_all(&admin)
    .await
    .expect("read finalization schema residue");
    println!("FINALIZATION_SCHEMA_RESIDUE_COUNT={}", schemas.len());
    for schema in schemas {
        println!("FINALIZATION_SCHEMA_RESIDUE={schema}");
    }
    admin.close().await;
}

#[tokio::test]
#[ignore = "creates and drops one random isolated schema; verifies fencing takeover, expired retry and transactional rollback"]
async fn atomic_finalization_rejects_stale_fencing_allows_confirmed_takeover_and_converges() {
    let config = config();
    let admin = pool(&config, None).await;
    create_schema(&admin, &config.schema).await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let workbench = pool(&config, Some(&config.schema)).await;
        WorkbenchStore::new(workbench.clone()).migrate().await?;
        let mac = "02AABBCCDDEE".to_string();
        sqlx::query(
            "INSERT INTO aio_node \
             (mac_normalized, name, ip, display_mac, management_state, source, version, created_at, updated_at) \
             VALUES (?, 'finalization-node', '192.0.2.10', '02:AA:BB:CC:DD:EE', \
                     'pending', 'finalization-test', 1, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
        )
        .bind(&mac)
        .execute(&workbench)
        .await?;
        let mut confirmed_asset = inxaiot_desk_buddy_lib::infrastructure::workbench_aio::WorkbenchAioRepository::new(workbench.clone())
            .list_snapshots().await?.remove(0);
        confirmed_asset.platform_aio_id = Some("2108257198208651265".into());
        confirmed_asset.name = "部署前旧名称不能覆盖已有资料".into();
        sqlx::query(
            "INSERT INTO aio_node_service_version \
             (mac_normalized, service_name, expected_image_name, expected_version) \
             VALUES (?, 'removed-service', 'removed', 'old')",
        )
        .bind(&mac)
        .execute(&workbench)
        .await?;
        let operations = OperationRepository::new(workbench.clone());
        let operation = operations
            .start(OperationStart {
                domain_type: "aio".into(),
                operation_type: "full_upgrade".into(),
                operation_name: "atomic finalization failure injection".into(),
                operator_name: "finalization-test".into(),
                instance_id: "finalization-instance".into(),
                targets: vec![("aio".into(), mac.clone())],
                artifact_name: Some("Release".into()),
                artifact_version: Some("finalization-test".into()),
                operation_summary: None,
                retry_of_operation_id: None,
            })
            .await?;
        let lease = ResourceLeaseRepository::new(workbench.clone())
            .acquire_many(vec![LeaseRequest {
                resource_type: "aio".into(),
                resource_key: mac.clone(),
                domain_type: "aio".into(),
                operation_id: operation.id.clone(),
                owner_instance_id: "finalization-instance".into(),
                owner_user: "finalization-test".into(),
                ttl: Duration::from_secs(120),
            }])
            .await?
            .remove(0);
        let mut write = AtomicDeploymentFinalization {
            operation: OperationFinalResult {
                operation_id: operation.id.clone(),
                expected_version: operation.version,
                state: "succeeded".into(),
                result_summary: Some("atomic success".into()),
                error_code: None,
                error_summary: None,
            },
            targets: vec![AtomicTargetFinalization {
                asset: Some(confirmed_asset),
                result: TargetFinalResult {
                    operation_id: operation.id.clone(),
                    resource_type: "aio".into(),
                    resource_key: mac.clone(),
                    result_state: "succeeded".into(),
                    before_version: Some("before".into()),
                    after_version: Some("after".into()),
                    result_summary: Some("healthy".into()),
                    error_code: None,
                    error_summary: None,
                },
                service_versions: vec![ServiceVersionWrite {
                    mac: mac.clone(),
                    service_name: "device-edge".into(),
                    expected_image_name: Some("device-edge".into()),
                    expected_version: Some("new".into()),
                    observed_image_name: None,
                    observed_version: None,
                    source_operation_id: Some(operation.id.clone()),
                }],
                replace_service_versions: true,
                mark_operation_success: true,
            }],
            leases: vec![lease.clone()],
        };

        let mut stale = write.clone();
        stale.leases[0].fencing_token = stale.leases[0].fencing_token.saturating_add(1);
        ensure(
            finalize_deployment_atomically(&workbench, stale)
                .await
                .is_err(),
            "stale fencing finalization unexpectedly succeeded",
        )?;
        verify_unchanged(&workbench, &operation.id, &mac).await?;

        let competing_operation = operations
            .start(OperationStart {
                domain_type: "aio".into(),
                operation_type: "full_upgrade".into(),
                operation_name: "competing finalization takeover".into(),
                operator_name: "competing-test".into(),
                instance_id: "competing-instance".into(),
                targets: vec![("aio".into(), mac.clone())],
                artifact_name: Some("Release".into()),
                artifact_version: Some("competing-test".into()),
                operation_summary: None,
                retry_of_operation_id: None,
            })
            .await?;
        let lease_repository = ResourceLeaseRepository::new(workbench.clone());
        let competing_lease = lease_repository
            .force_acquire_many(vec![LeaseRequest {
                resource_type: "aio".into(),
                resource_key: mac.clone(),
                domain_type: "aio".into(),
                operation_id: competing_operation.id.clone(),
                owner_instance_id: "competing-instance".into(),
                owner_user: "competing-test".into(),
                ttl: Duration::from_secs(120),
            }])
            .await?
            .remove(0);
        ensure(
            competing_lease.fencing_token > lease.fencing_token,
            "forced takeover did not advance fencing",
        )?;
        ensure(
            finalize_deployment_atomically(&workbench, write.clone())
                .await
                .is_err(),
            "taken-over finalization unexpectedly succeeded",
        )?;
        let recovered_lease = lease_repository
            .force_acquire_many(vec![LeaseRequest {
                resource_type: "aio".into(),
                resource_key: mac.clone(),
                domain_type: "aio".into(),
                operation_id: operation.id.clone(),
                owner_instance_id: "recovery-instance".into(),
                owner_user: "recovery-test".into(),
                ttl: Duration::from_secs(120),
            }])
            .await?
            .remove(0);
        ensure(
            recovered_lease.fencing_token > competing_lease.fencing_token,
            "confirmed recovery did not advance fencing",
        )?;
        write.leases = vec![recovered_lease];

        sqlx::raw_sql(
            "CREATE TRIGGER fail_atomic_node_update BEFORE UPDATE ON aio_node \
             FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'FINALIZATION_INJECTED'",
        )
        .execute(&workbench)
        .await?;
        ensure(
            finalize_deployment_atomically(&workbench, write.clone())
                .await
                .is_err(),
            "injected transactional failure unexpectedly succeeded",
        )?;
        verify_unchanged(&workbench, &operation.id, &mac).await?;
        sqlx::raw_sql("DROP TRIGGER fail_atomic_node_update")
            .execute(&workbench)
            .await?;

        sqlx::query(
            "UPDATE resource_lease SET expires_at = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND) \
             WHERE resource_type = 'aio' AND resource_key = ?",
        )
        .bind(&mac)
        .execute(&workbench)
        .await?;

        finalize_deployment_atomically(&workbench, write.clone()).await?;
        ensure(
            shared_operation_state(&workbench, &operation.id).await?.as_deref()
                == Some("succeeded"),
            "shared operation did not converge to succeeded",
        )?;
        ensure(
            finalize_deployment_atomically(&workbench, write).await.is_err(),
            "duplicate finalization unexpectedly succeeded",
        )?;
        let row = sqlx::query(
            "SELECT o.state AS operation_state, o.success_count, t.result_state AS target_state, \
                    n.management_state, n.version AS node_version, l.lease_state \
             FROM operation_record o \
             JOIN operation_target_result t ON t.operation_id = o.id \
             JOIN aio_node n ON n.mac_normalized = t.resource_key \
             JOIN resource_lease l ON l.operation_id = o.id \
             WHERE o.id = ?",
        )
        .bind(&operation.id)
        .fetch_one(&workbench)
        .await?;
        ensure(row.try_get::<String, _>("operation_state")? == "succeeded", "operation state")?;
        ensure(row.try_get::<u32, _>("success_count")? == 1, "success count")?;
        ensure(row.try_get::<String, _>("target_state")? == "succeeded", "target state")?;
        ensure(row.try_get::<String, _>("management_state")? == "managed", "node state")?;
        ensure(row.try_get::<u64, _>("node_version")? == 2, "node version")?;
        ensure(row.try_get::<String, _>("lease_state")? == "released", "lease state")?;
        let linked: (Option<String>, String) = sqlx::query_as("SELECT platform_aio_id,name FROM aio_node WHERE mac_normalized=?")
            .bind(&mac).fetch_one(&workbench).await?;
        ensure(linked.0.as_deref() == Some("2108257198208651265"), "confirmed platform ID was not saved")?;
        ensure(linked.1 == "finalization-node", "existing asset metadata was overwritten")?;
        let services = sqlx::query_scalar::<_, String>(
            "SELECT service_name FROM aio_node_service_version WHERE mac_normalized = ? \
             ORDER BY service_name",
        )
        .bind(&mac)
        .fetch_all(&workbench)
        .await?;
        ensure(
            services == vec!["device-edge".to_string()],
            "full finalization did not replace stale service versions",
        )?;
        workbench.close().await;
        Ok(())
    }
    .await;
    drop_schema(&admin, &config.schema).await;
    admin.close().await;
    result.expect("atomic finalization real gate");
}

async fn verify_unchanged(
    pool: &MySqlPool,
    operation_id: &str,
    mac: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let row = sqlx::query(
        "SELECT o.state AS operation_state, o.success_count, t.result_state AS target_state, \
                n.management_state, n.version AS node_version, l.lease_state \
         FROM operation_record o \
         JOIN operation_target_result t ON t.operation_id = o.id \
         JOIN aio_node n ON n.mac_normalized = ? \
         JOIN resource_lease l ON l.operation_id = o.id \
         WHERE o.id = ?",
    )
    .bind(mac)
    .bind(operation_id)
    .fetch_one(pool)
    .await?;
    ensure(
        row.try_get::<String, _>("operation_state")? == "running",
        "operation changed",
    )?;
    ensure(
        row.try_get::<u32, _>("success_count")? == 0,
        "count changed",
    )?;
    ensure(
        row.try_get::<String, _>("target_state")? == "pending",
        "target changed",
    )?;
    ensure(
        row.try_get::<String, _>("management_state")? == "pending",
        "node changed",
    )?;
    ensure(
        row.try_get::<u64, _>("node_version")? == 1,
        "node version changed",
    )?;
    let platform_id: Option<String> = sqlx::query_scalar("SELECT platform_aio_id FROM aio_node WHERE mac_normalized=?")
        .bind(mac).fetch_one(pool).await?;
    ensure(platform_id.is_none(), "platform ID changed before transaction commit")?;
    ensure(
        row.try_get::<String, _>("lease_state")? == "active",
        "lease changed",
    )?;
    let services = sqlx::query_scalar::<_, String>(
        "SELECT service_name FROM aio_node_service_version WHERE mac_normalized = ?",
    )
    .bind(mac)
    .fetch_all(pool)
    .await?;
    ensure(
        services == vec!["removed-service".to_string()],
        "service versions changed before transaction commit",
    )?;
    Ok(())
}
