use std::path::Path;
use std::time::Duration;

use inxaiot_desk_buddy_lib::formal::error::FormalError;
use inxaiot_desk_buddy_lib::formal::mysql::{MySqlConnectionSpec, MySqlTlsMode, ProjectMySqlPools};
use inxaiot_desk_buddy_lib::formal::resource_lease_repository::{
    LeaseRequest, ResourceLeaseRepository,
};
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
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

fn config() -> MySqlConnectionSpec {
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
    MySqlConnectionSpec {
        host: default(&yaml, "MYSQL_HOST").into(),
        port: default(&yaml, "MYSQL_PORT").parse().expect("mysql port"),
        username: default(&yaml, "MYSQL_USER").into(),
        password: default(&yaml, "MYSQL_PASSWORD").into(),
        platform_schema: line(&description, "平台业务数据库名：").into(),
        workbench_schema: line(&description, "工作台数据库：").into(),
        tls_mode: MySqlTlsMode::Disabled,
        connect_timeout: Duration::from_secs(10),
    }
}

fn request(resource_key: &str, owner: &str, operation_id: &str) -> LeaseRequest {
    LeaseRequest {
        resource_type: "aio".into(),
        resource_key: resource_key.into(),
        domain_type: "aio".into(),
        operation_id: operation_id.into(),
        owner_instance_id: owner.into(),
        owner_user: format!("{owner}-user"),
        ttl: Duration::from_secs(30),
    }
}

#[tokio::test]
#[ignore = "writes and removes isolated released lease counter rows"]
async fn released_row_preserves_monotonic_fencing_token() {
    let config = config();
    let pools = ProjectMySqlPools::connect(&config)
        .await
        .expect("connect project mysql");
    WorkbenchStore::new(pools.workbench().clone())
        .migrate()
        .await
        .expect("workbench migration");
    let leases = ResourceLeaseRepository::new(pools.workbench().clone());
    let resource_key = format!("poc:lease:{}", Uuid::now_v7());
    let first_operation = Uuid::now_v7().to_string();
    let second_operation = Uuid::now_v7().to_string();

    let first = leases
        .acquire_many(vec![request(&resource_key, "instance-a", &first_operation)])
        .await
        .expect("first lease")
        .remove(0);
    assert_eq!(first.fencing_token, 1);
    assert!(
        leases
            .validate_fencing(&first)
            .await
            .expect("first fencing")
    );
    leases
        .heartbeat(&first, Duration::from_secs(30))
        .await
        .expect("first heartbeat");

    let same_instance_busy = leases
        .acquire_many(vec![request(
            &resource_key,
            "instance-a",
            &second_operation,
        )])
        .await;
    assert!(matches!(same_instance_busy, Err(FormalError::Conflict(_))));

    let busy = leases
        .acquire_many(vec![request(
            &resource_key,
            "instance-b",
            &second_operation,
        )])
        .await;
    assert!(matches!(busy, Err(FormalError::Conflict(_))));

    leases.release(&first).await.expect("release first lease");
    assert!(
        !leases
            .validate_fencing(&first)
            .await
            .expect("stale fencing")
    );
    let second = leases
        .acquire_many(vec![request(
            &resource_key,
            "instance-b",
            &second_operation,
        )])
        .await
        .expect("second lease")
        .remove(0);
    assert!(second.fencing_token > first.fencing_token);
    assert!(
        leases
            .validate_fencing(&second)
            .await
            .expect("second fencing")
    );
    leases.release(&second).await.expect("release second lease");
    leases
        .delete_test_counter("aio", &resource_key)
        .await
        .expect("delete exact test counter");
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM resource_lease WHERE resource_type = 'aio' AND resource_key = ?",
    )
    .bind(&resource_key)
    .fetch_one(pools.workbench())
    .await
    .expect("verify lease cleanup");
    assert_eq!(remaining, 0);
    pools.close().await;
}
