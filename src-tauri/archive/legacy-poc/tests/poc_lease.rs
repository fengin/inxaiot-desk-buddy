#[path = "../src/core/mod.rs"]
mod core;
#[path = "../src/infrastructure/database_v2.rs"]
mod database;
#[path = "../src/infrastructure/lease_poc.rs"]
mod lease;

use std::path::Path;
use std::time::Duration;

use core::secret::SecretValue;
use database::{DatabaseTlsMode, DualMySqlPools, MySqlProjectConfig};
use lease::{AcquireLeaseOutcome, MySqlLeasePoc};

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
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("project");
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
        connect_timeout: Duration::from_secs(10),
        tls_mode: DatabaseTlsMode::Disabled,
    }
}

#[tokio::test]
#[ignore = "creates and removes isolated PoC tables in the authorized workbench database"]
async fn two_instances_use_lease_and_fencing_token() {
    let config = config();
    let pools = DualMySqlPools::connect(&config)
        .await
        .expect("connect mysql");
    let leases = MySqlLeasePoc::new(pools.workbench.clone());
    leases.prepare().await.expect("prepare poc tables");
    let resource = "aio:00:0c:29:poc";

    let first = match leases
        .acquire(resource, "instance-a", Duration::from_secs(30))
        .await
        .expect("first acquire")
    {
        AcquireLeaseOutcome::Acquired(grant) => grant,
        other => panic!("unexpected first outcome: {other:?}"),
    };
    let busy = leases
        .acquire(resource, "instance-b", Duration::from_secs(30))
        .await
        .expect("second acquire");
    assert!(matches!(busy, AcquireLeaseOutcome::Busy { .. }));

    leases
        .force_expire_for_test(resource)
        .await
        .expect("expire first lease");
    let second = match leases
        .acquire(resource, "instance-b", Duration::from_secs(30))
        .await
        .expect("take expired lease")
    {
        AcquireLeaseOutcome::Acquired(grant) => grant,
        other => panic!("unexpected takeover outcome: {other:?}"),
    };
    assert!(second.fencing_token > first.fencing_token);
    assert!(
        !leases
            .write_fenced_result(&first, "stale")
            .await
            .expect("reject stale result")
    );
    assert!(
        leases
            .write_fenced_result(&second, "current")
            .await
            .expect("accept current result")
    );
    leases.cleanup().await.expect("cleanup poc tables");
    pools.close().await;
}

