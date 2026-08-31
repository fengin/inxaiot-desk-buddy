use std::path::Path;
use std::time::{Duration, Instant};

use inxaiot_desk_buddy_lib::domain::aio::inventory::{ImportClassification, reconcile_inventory};
use inxaiot_desk_buddy_lib::infrastructure::csv_inventory::parse_inventory_path;

fn inventory_fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project root")
        .join("test/fixtures/inventory")
}

#[test]
fn csv_golden_and_branch_samples_keep_compatible_validation() {
    let fixtures = inventory_fixtures();
    let golden =
        parse_inventory_path(&fixtures.join("inventory.test.csv")).expect("golden inventory");
    assert_eq!(golden.len(), 2);
    assert!(golden.iter().all(|row| row.errors.is_empty()));

    assert!(parse_inventory_path(&fixtures.join("inventory-empty.csv")).is_err());
    assert!(parse_inventory_path(&fixtures.join("inventory-missing-mac.csv")).is_err());
    let duplicate_ip = parse_inventory_path(&fixtures.join("inventory-duplicate-ip.csv"))
        .expect("duplicate IP rows");
    let reconciled = reconcile_inventory(duplicate_ip, &[], &[]);
    assert!(
        reconciled
            .iter()
            .all(|item| item.classification == ImportClassification::Invalid)
    );
}

#[test]
fn three_hundred_node_sample_parses_and_reconciles_within_budget() {
    let started = Instant::now();
    let rows = parse_inventory_path(&inventory_fixtures().join("inventory.300.test.csv"))
        .expect("300 node inventory");
    assert_eq!(rows.len(), 300);
    let items = reconcile_inventory(rows, &[], &[]);
    assert_eq!(items.len(), 300);
    assert!(
        items
            .iter()
            .all(|item| item.classification == ImportClassification::NewPending)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}
