#[path = "common/aio_test_config.rs"]
mod aio_test_config;

use inxaiot_desk_buddy_lib::domain::aio::inventory::ImportCounts;
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use inxaiot_desk_buddy_lib::infrastructure::database::DualMySqlPools;
use inxaiot_desk_buddy_lib::infrastructure::platform_aio::PlatformAioRepository;
use inxaiot_desk_buddy_lib::infrastructure::workbench_aio::{
    ApplyInventoryWrite, InventoryAssetWrite, WorkbenchAioRepository,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

fn isolated_mac() -> String {
    let uuid = Uuid::now_v7().simple().to_string().to_uppercase();
    format!("02{}", &uuid[uuid.len() - 10..])
}

fn platform_identity_hash(
    snapshot: &inxaiot_desk_buddy_lib::infrastructure::platform_aio::PlatformInventorySnapshot,
) -> Vec<u8> {
    let identity = snapshot
        .nodes
        .iter()
        .map(|node| {
            (
                &node.id,
                &node.name,
                &node.ip,
                &node.mac_raw,
                &node.building_id,
                &node.addr_alias,
            )
        })
        .collect::<Vec<_>>();
    Sha256::digest(serde_json::to_vec(&identity).expect("serialize platform identities")).to_vec()
}

fn asset(mac: String, ip_suffix: u8) -> InventoryAssetWrite {
    InventoryAssetWrite {
        display_mac: mac
            .as_bytes()
            .chunks(2)
            .map(|chunk| std::str::from_utf8(chunk).expect("ASCII"))
            .collect::<Vec<_>>()
            .join(":"),
        mac_normalized: mac,
        name: format!("阶段5精确清理测试-{ip_suffix}"),
        ip: format!("192.0.2.{ip_suffix}"),
        building_id: None,
        region_id: None,
        addr_alias: Some("阶段5测试位置".into()),
        floor: None,
        location: None,
        remark: None,
        platform_aio_id: None,
        management_state: "pending".into(),
        source: "import".into(),
        expected_version: None,
    }
}

#[tokio::test]
#[ignore = "requires authorized project databases and writes exact isolated assets"]
async fn platform_is_read_only_and_import_application_is_atomic_and_exactly_cleaned() {
    let pools = DualMySqlPools::connect(&aio_test_config::isolated_project())
        .await
        .expect("connect project databases");
    WorkbenchStore::new(pools.workbench.clone())
        .migrate()
        .await
        .expect("workbench migration");
    let platform_repository = PlatformAioRepository::new(pools.platform.clone());
    let before_platform = platform_repository
        .list_all()
        .await
        .expect("platform snapshot");
    assert!(!before_platform.nodes.is_empty());
    let before_hash = platform_identity_hash(&before_platform);

    let repository = WorkbenchAioRepository::new(pools.workbench.clone());
    let macs = [isolated_mac(), isolated_mac()];
    assert_ne!(macs[0], macs[1]);
    let result = repository
        .apply_inventory(ApplyInventoryWrite {
            file_name: "phase5-real.csv".into(),
            operator_name: "phase5-test".into(),
            instance_id: "phase5-real-test".into(),
            classification_counts: serde_json::to_value(ImportCounts {
                total: 2,
                new_pending: 2,
                selected: 2,
                ..ImportCounts::default()
            })
            .expect("counts"),
            assets: vec![asset(macs[0].clone(), 51), asset(macs[1].clone(), 52)],
        })
        .await
        .expect("apply inventory");
    println!(
        "phase5 operation_id={} macs={},{}",
        result.operation_id, macs[0], macs[1]
    );

    let node_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM aio_node WHERE mac_normalized IN (?, ?) AND last_operation_id = ?",
    )
    .bind(&macs[0])
    .bind(&macs[1])
    .bind(&result.operation_id)
    .fetch_one(&pools.workbench)
    .await
    .expect("node count");
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_event WHERE object_type = 'aio_node' \
         AND object_key IN (?, ?) AND instance_id = 'phase5-real-test'",
    )
    .bind(&macs[0])
    .bind(&macs[1])
    .fetch_one(&pools.workbench)
    .await
    .expect("audit count");
    assert_eq!(node_count, 2);
    assert_eq!(audit_count, 2);

    let duplicate = repository
        .apply_inventory(ApplyInventoryWrite {
            file_name: "phase5-duplicate.csv".into(),
            operator_name: "phase5-test".into(),
            instance_id: "phase5-real-test".into(),
            classification_counts: serde_json::json!({ "selected": 2 }),
            assets: vec![asset(macs[0].clone(), 51), asset(macs[1].clone(), 52)],
        })
        .await;
    assert!(duplicate.is_err());
    let duplicate_operation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM operation_record \
         WHERE instance_id = 'phase5-real-test' AND artifact_name = 'phase5-duplicate.csv'",
    )
    .fetch_one(&pools.workbench)
    .await
    .expect("rolled back operation count");
    assert_eq!(duplicate_operation_count, 0);

    let after_platform = platform_repository
        .list_all()
        .await
        .expect("platform snapshot after");
    let after_hash = platform_identity_hash(&after_platform);
    assert_eq!(before_hash, after_hash);

    let mut cleanup = pools.workbench.begin().await.expect("cleanup transaction");
    sqlx::query(
        "DELETE FROM audit_event WHERE object_type = 'aio_node' \
         AND object_key IN (?, ?) AND instance_id = 'phase5-real-test'",
    )
    .bind(&macs[0])
    .bind(&macs[1])
    .execute(&mut *cleanup)
    .await
    .expect("cleanup audits");
    sqlx::query("DELETE FROM aio_node WHERE mac_normalized IN (?, ?) AND last_operation_id = ?")
        .bind(&macs[0])
        .bind(&macs[1])
        .bind(&result.operation_id)
        .execute(&mut *cleanup)
        .await
        .expect("cleanup nodes");
    sqlx::query("DELETE FROM operation_record WHERE id = ? AND instance_id = 'phase5-real-test'")
        .bind(&result.operation_id)
        .execute(&mut *cleanup)
        .await
        .expect("cleanup operation");
    cleanup.commit().await.expect("commit cleanup");

    let remaining: i64 = sqlx::query(
        "SELECT \
           (SELECT COUNT(*) FROM aio_node WHERE mac_normalized IN (?, ?)) + \
           (SELECT COUNT(*) FROM audit_event WHERE object_type = 'aio_node' \
              AND object_key IN (?, ?) AND instance_id = 'phase5-real-test') + \
           (SELECT COUNT(*) FROM operation_record WHERE id = ?) AS remaining",
    )
    .bind(&macs[0])
    .bind(&macs[1])
    .bind(&macs[0])
    .bind(&macs[1])
    .bind(&result.operation_id)
    .fetch_one(&pools.workbench)
    .await
    .expect("verify cleanup")
    .try_get("remaining")
    .expect("remaining");
    assert_eq!(remaining, 0);
    pools.close().await;
}
