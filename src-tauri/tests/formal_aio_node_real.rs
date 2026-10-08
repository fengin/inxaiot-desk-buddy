#[path = "common/aio_test_config.rs"]
mod aio_test_config;

use inxaiot_desk_buddy_lib::formal::aio_node_repository::{
    AioNodeRepository, AioNodeValues, AioNodeWrite, ServiceVersionWrite,
};
use inxaiot_desk_buddy_lib::formal::error::FormalError;
use inxaiot_desk_buddy_lib::formal::mac;
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use inxaiot_desk_buddy_lib::infrastructure::database::DualMySqlPools;
use sqlx::Row;
use uuid::Uuid;

fn values(mac: &str, name: &str) -> AioNodeValues {
    AioNodeValues {
        mac: mac.into(),
        name: name.into(),
        ip: "192.0.2.31".into(),
        building_id: Some("poc-building".into()),
        region_id: None,
        addr_alias: Some("poc-location".into()),
        floor: Some("1F".into()),
        location: Some("PoC room".into()),
        remark: None,
        platform_aio_id: None,
        management_state: "pending".into(),
        source: "import".into(),
    }
}

#[test]
fn mac_normalization_is_the_only_asset_identity() {
    assert_eq!(
        mac::normalize_mac("00:0c:29:3b:b9:31").expect("normalize mac"),
        "000C293BB931"
    );
    assert_eq!(
        mac::normalize_mac("00-0C-29-3B-B9-31").expect("normalize mac"),
        "000C293BB931"
    );
}

#[tokio::test]
#[ignore = "writes and removes one isolated aio node"]
async fn aio_asset_optimistic_lock_service_version_audit_and_cleanup() {
    let config = aio_test_config::isolated_project();
    let pools = DualMySqlPools::connect(&config)
        .await
        .expect("connect project mysql");
    WorkbenchStore::new(pools.workbench.clone())
        .migrate()
        .await
        .expect("workbench migration");
    let repository = AioNodeRepository::new(pools.workbench.clone());
    let mac = Uuid::now_v7().simple().to_string()[..12].to_uppercase();

    let first = repository
        .save(AioNodeWrite {
            values: values(&mac, "PoC node v1"),
            expected_version: None,
            action: "create".into(),
            operator_name: "phase3-test".into(),
            instance_id: "instance-a".into(),
        })
        .await
        .expect("create aio node");
    assert_eq!(first.version, 1);
    assert_eq!(first.mac_normalized, mac);

    let stale = repository
        .save(AioNodeWrite {
            values: values(&mac, "stale update"),
            expected_version: Some(0),
            action: "update".into(),
            operator_name: "phase3-test".into(),
            instance_id: "instance-b".into(),
        })
        .await;
    assert!(matches!(stale, Err(FormalError::Conflict(_))));

    let second = repository
        .save(AioNodeWrite {
            values: values(&mac, "PoC node v2"),
            expected_version: Some(1),
            action: "update".into(),
            operator_name: "phase3-test".into(),
            instance_id: "instance-b".into(),
        })
        .await
        .expect("update aio node");
    assert_eq!(second.version, 2);
    assert_eq!(second.name, "PoC node v2");

    repository
        .save_service_version(ServiceVersionWrite {
            mac: mac.clone(),
            service_name: "device-edge".into(),
            expected_image_name: Some("device-edge".into()),
            expected_version: Some("poc-1".into()),
            observed_image_name: Some("device-edge".into()),
            observed_version: Some("poc-1".into()),
            source_operation_id: None,
        })
        .await
        .expect("save service version");
    let service_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM aio_node_service_version WHERE mac_normalized = ?",
    )
    .bind(&mac)
    .fetch_one(&pools.workbench)
    .await
    .expect("service version count");
    assert_eq!(service_count, 1);

    let audit_rows = sqlx::query(
        "SELECT CAST(changed_fields_json AS CHAR) AS changed_fields_text FROM audit_event \
         WHERE object_type = 'aio_node' AND object_key = ?",
    )
    .bind(&mac)
    .fetch_all(&pools.workbench)
    .await
    .expect("aio node audit");
    assert_eq!(audit_rows.len(), 2);
    for row in audit_rows {
        let fields: String = row.try_get("changed_fields_text").expect("changed fields");
        assert!(fields.contains("management_state"));
        assert!(!fields.contains("PoC node"));
    }

    repository
        .delete_test_node(&mac)
        .await
        .expect("cleanup aio node");
    let remaining: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM aio_node WHERE mac_normalized = ?) + \
         (SELECT COUNT(*) FROM audit_event WHERE object_type = 'aio_node' AND object_key = ?)",
    )
    .bind(&mac)
    .bind(&mac)
    .fetch_one(&pools.workbench)
    .await
    .expect("verify aio cleanup");
    assert_eq!(remaining, 0);
    pools.close().await;
}
