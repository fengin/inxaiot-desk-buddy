use super::*;
use sqlx::{Sqlite, SqlitePool, sqlite::SqlitePoolOptions};

struct SqliteAliveScreenLookup<'tx, 'connection> {
    tx: &'tx mut Transaction<'connection, Sqlite>,
    batches: Vec<Vec<String>>,
    drop_table_on_call: Option<usize>,
}

impl AliveScreenLookup for SqliteAliveScreenLookup<'_, '_> {
    async fn fetch_alive(&mut self, ids: &[String]) -> AppResult<Vec<String>> {
        self.batches.push(ids.to_vec());
        if self.drop_table_on_call == Some(self.batches.len()) {
            sqlx::query("DROP TABLE smart_terminal_screen")
                .execute(&mut **self.tx)
                .await
                .map_err(db)?;
        }
        alive_id_query::<Sqlite>(ids)
            .build_query_scalar()
            .fetch_all(&mut **self.tx)
            .await
            .map_err(db)
    }
}

fn lookup<'tx, 'connection>(
    tx: &'tx mut Transaction<'connection, Sqlite>,
) -> SqliteAliveScreenLookup<'tx, 'connection> {
    SqliteAliveScreenLookup {
        tx,
        batches: Vec::new(),
        drop_table_on_call: None,
    }
}

fn asset(id: &str) -> ScreenAsset {
    ScreenAsset {
        id: id.into(),
        ..Default::default()
    }
}

async fn fixture() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE smart_terminal_screen(id TEXT PRIMARY KEY, delete_flag INTEGER NOT NULL, building_id TEXT)").execute(&pool).await.unwrap();
    sqlx::query("CREATE TABLE t_project_building(id TEXT PRIMARY KEY, project_info_id TEXT, delete_flag INTEGER NOT NULL)").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO t_project_building VALUES ('here','current',0),('elsewhere','other',0),('invalid','current',1)").execute(&pool).await.unwrap();
    pool
}

async fn insert(pool: &SqlitePool, id: &str, deleted: i32, building: &str) {
    sqlx::query("INSERT INTO smart_terminal_screen VALUES (?,?,?)")
        .bind(id)
        .bind(deleted)
        .bind(building)
        .execute(pool)
        .await
        .unwrap();
}

async fn returned_in_current_project(tx: &mut Transaction<'_, Sqlite>) -> Vec<ScreenAsset> {
    let ids: Vec<String> = sqlx::query_scalar("SELECT s.id FROM smart_terminal_screen s JOIN t_project_building b ON b.id=s.building_id WHERE s.delete_flag=0 AND b.delete_flag=0 AND b.project_info_id='current' ORDER BY s.id")
        .fetch_all(&mut **tx).await.unwrap();
    ids.iter().map(|id| asset(id)).collect()
}

#[tokio::test]
async fn all_269_known_screens_returned_require_no_additional_queries() {
    let pool = fixture().await;
    let known = (1..=269).map(|id| id.to_string()).collect::<Vec<_>>();
    for id in &known {
        insert(&pool, id, 0, "here").await;
    }
    let mut tx = pool.begin().await.unwrap();
    let returned = returned_in_current_project(&mut tx).await;
    assert_eq!(returned.len(), 269);
    let mut lookup = lookup(&mut tx);
    assert!(
        deleted_known_ids(&known, &returned, &mut lookup)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(lookup.batches.is_empty());
}

#[tokio::test]
async fn only_main_table_absence_or_logical_deletion_counts_as_deleted() {
    let pool = fixture().await;
    insert(&pool, "present", 0, "here").await;
    insert(&pool, "moved", 0, "elsewhere").await;
    insert(&pool, "invalid-space", 0, "invalid").await;
    insert(&pool, "missing-space", 0, "nonexistent").await;
    insert(&pool, "logically-deleted", 1, "here").await;
    insert(&pool, "physically-deleted", 0, "here").await;
    sqlx::query("DELETE FROM smart_terminal_screen WHERE id='physically-deleted'")
        .execute(&pool)
        .await
        .unwrap();
    let known = [
        "present",
        "moved",
        "logically-deleted",
        "invalid-space",
        "physically-deleted",
        "missing-space",
    ]
    .map(String::from);
    let mut tx = pool.begin().await.unwrap();
    let returned = returned_in_current_project(&mut tx).await;
    assert_eq!(
        returned.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["present"]
    );
    let mut lookup = lookup(&mut tx);
    let deleted = deleted_known_ids(&known, &returned, &mut lookup)
        .await
        .unwrap();
    assert_eq!(deleted, ["logically-deleted", "physically-deleted"]);
    assert_eq!(lookup.batches, [known[1..].to_vec()]);
}

#[tokio::test]
async fn duplicates_are_checked_once_and_only_known_ids_can_be_deleted() {
    let pool = fixture().await;
    insert(&pool, "alive", 0, "elsewhere").await;
    let known = ["returned", "gone", "returned", "alive", "gone", "alive"].map(String::from);
    let returned = [asset("returned"), asset("new-not-in-known-list")];
    let mut tx = pool.begin().await.unwrap();
    let mut lookup = lookup(&mut tx);
    assert_eq!(
        deleted_known_ids(&known, &returned, &mut lookup)
            .await
            .unwrap(),
        ["gone"]
    );
    assert_eq!(
        lookup.batches,
        [vec!["gone".to_string(), "alive".to_string()]]
    );
}

#[tokio::test]
async fn empty_known_ids_never_issue_an_empty_in_query() {
    let pool = fixture().await;
    let mut tx = pool.begin().await.unwrap();
    let mut lookup = lookup(&mut tx);
    assert!(
        deleted_known_ids(&[], &[asset("new")], &mut lookup)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(lookup.batches.is_empty());
}

#[tokio::test]
async fn query_batches_respect_500_id_boundaries_and_preserve_deletion_order() {
    let pool = fixture().await;
    for count in [1_usize, 499, 500, 501, 1000, 1001] {
        let known = (1..=count).map(|id| id.to_string()).collect::<Vec<_>>();
        let mut tx = pool.begin().await.unwrap();
        let mut lookup = lookup(&mut tx);
        assert_eq!(
            deleted_known_ids(&known, &[], &mut lookup).await.unwrap(),
            known
        );
        assert_eq!(lookup.batches.len(), count.div_ceil(ALIVE_ID_BATCH_SIZE));
        assert!(
            lookup
                .batches
                .iter()
                .all(|batch| !batch.is_empty() && batch.len() <= ALIVE_ID_BATCH_SIZE)
        );
        assert_eq!(lookup.batches.concat(), known);
    }
}

#[tokio::test]
async fn ids_with_sql_metacharacters_remain_bound_values() {
    let pool = fixture().await;
    let literal = "x') OR 1=1 --";
    insert(&pool, literal, 0, "elsewhere").await;
    insert(&pool, "other-alive", 0, "here").await;
    let ids = [literal.to_string()];
    let mut tx = pool.begin().await.unwrap();
    let alive: Vec<String> = alive_id_query::<Sqlite>(&ids)
        .build_query_scalar()
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(alive, ids);
}

#[tokio::test]
async fn later_query_failure_never_returns_partial_deletion_conclusions() {
    let pool = fixture().await;
    let known = (1..=501).map(|id| id.to_string()).collect::<Vec<_>>();
    let mut tx = pool.begin().await.unwrap();
    let mut lookup = lookup(&mut tx);
    lookup.drop_table_on_call = Some(2);
    // 第一块查不到任何记录；第二块让数据库真正报错，不能返回第一块的500条删除结论。
    assert!(deleted_known_ids(&known, &[], &mut lookup).await.is_err());
    assert_eq!(
        lookup.batches.iter().map(Vec::len).collect::<Vec<_>>(),
        [500, 1]
    );
}
