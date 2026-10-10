use std::collections::HashMap;

use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::assets::{AioImportSession, ImportSelection};
use crate::domain::aio::inventory::{ImportCounts, ReconciledImportItem};

#[derive(Clone)]
pub struct AioImportRepository {
    pool: SqlitePool,
}

impl AioImportRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn create_preview(
        &self,
        local_project_id: &str,
        file_name: &str,
        file_path: &str,
        items: &[ReconciledImportItem],
    ) -> AppResult<AioImportSession> {
        if local_project_id.trim().is_empty()
            || file_name.trim().is_empty()
            || file_path.trim().is_empty()
        {
            return Err(AppError::InvalidConfig(
                "导入项目、文件名和文件路径不能为空".into(),
            ));
        }
        let id = Uuid::now_v7().to_string();
        let now = timestamp();
        let counts = ImportCounts::from_items(items);
        let counts_json = serde_json::to_string(&counts)
            .map_err(|_| AppError::InvalidConfig("导入统计无法序列化".into()))?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| AppError::database("开始本地导入事务", &error))?;
        sqlx::query(
            "INSERT INTO local_aio_import_session \
             (id, local_project_id, file_name, file_path, state, counts_json, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'preview', ?, ?, ?)",
        )
        .bind(&id)
        .bind(local_project_id)
        .bind(file_name)
        .bind(file_path)
        .bind(&counts_json)
        .bind(&now)
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| AppError::database("创建本地导入会话", &error))?;
        for item in items {
            let item_json = serde_json::to_string(item)
                .map_err(|_| AppError::InvalidConfig("导入行无法序列化".into()))?;
            let conflict_json = (!item.conflicts.is_empty())
                .then(|| serde_json::to_string(&item.conflicts))
                .transpose()
                .map_err(|_| AppError::InvalidConfig("导入冲突无法序列化".into()))?;
            let error = (!item.errors.is_empty()).then(|| item.errors.join("\n"));
            sqlx::query(
                "INSERT INTO local_aio_import_item \
                 (import_session_id, row_number, mac_normalized, parsed_data_json, classification, \
                  conflict_json, selected, error) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(i64::from(item.row_number))
            .bind(&item.mac_normalized)
            .bind(item_json)
            .bind(item.classification.as_str())
            .bind(conflict_json)
            .bind(i64::from(item.selected))
            .bind(error)
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("保存本地导入行", &error))?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交本地导入事务", &error))?;
        self.get(&id).await
    }

    pub async fn get(&self, session_id: &str) -> AppResult<AioImportSession> {
        let session = sqlx::query(
            "SELECT id, local_project_id, file_name, file_path, state, counts_json, \
                    created_at, updated_at FROM local_aio_import_session WHERE id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::database("读取本地导入会话", &error))?
        .ok_or_else(|| AppError::NotFound(format!("导入会话不存在：{session_id}")))?;
        let rows = sqlx::query(
            "SELECT parsed_data_json, selected FROM local_aio_import_item \
             WHERE import_session_id = ? ORDER BY row_number",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("读取本地导入行", &error))?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let json: String = row
                .try_get("parsed_data_json")
                .map_err(|error| AppError::database("解析本地导入行", &error))?;
            let mut item = serde_json::from_str::<ReconciledImportItem>(&json)
                .map_err(|_| AppError::InvalidConfig("本地导入行内容已损坏".into()))?;
            let selected: i64 = row
                .try_get("selected")
                .map_err(|error| AppError::database("解析本地导入选择状态", &error))?;
            item.selected = match selected {
                0 => false,
                1 => true,
                _ => {
                    return Err(AppError::InvalidConfig("本地导入选择状态已损坏".into()));
                }
            };
            items.push(item);
        }
        let counts_json: String = session
            .try_get("counts_json")
            .map_err(|error| AppError::database("解析本地导入统计", &error))?;
        Ok(AioImportSession {
            id: session
                .try_get("id")
                .map_err(|error| AppError::database("解析本地导入会话ID", &error))?,
            local_project_id: session
                .try_get("local_project_id")
                .map_err(|error| AppError::database("解析本地导入项目ID", &error))?,
            file_name: session
                .try_get("file_name")
                .map_err(|error| AppError::database("解析本地导入文件名", &error))?,
            file_path: session
                .try_get("file_path")
                .map_err(|error| AppError::database("解析本地导入文件路径", &error))?,
            state: session
                .try_get("state")
                .map_err(|error| AppError::database("解析本地导入状态", &error))?,
            counts: serde_json::from_str(&counts_json)
                .map_err(|_| AppError::InvalidConfig("本地导入统计已损坏".into()))?,
            created_at: session
                .try_get("created_at")
                .map_err(|error| AppError::database("解析本地导入创建时间", &error))?,
            updated_at: session
                .try_get("updated_at")
                .map_err(|error| AppError::database("解析本地导入更新时间", &error))?,
            items,
        })
    }

    pub async fn latest_open_for_project(
        &self,
        local_project_id: &str,
    ) -> AppResult<Option<AioImportSession>> {
        let id = sqlx::query_scalar::<_, String>(
            "SELECT id FROM local_aio_import_session \
             WHERE local_project_id = ? AND state = 'preview' \
             ORDER BY updated_at DESC, id DESC LIMIT 1",
        )
        .bind(local_project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::database("读取项目未处理导入会话", &error))?;
        match id {
            Some(id) => self.get(&id).await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn update_selection(
        &self,
        session_id: &str,
        selections: &[ImportSelection],
    ) -> AppResult<AioImportSession> {
        let current = self.get(session_id).await?;
        if current.state != "preview" {
            return Err(AppError::Conflict("导入会话已经结束，不能修改选择".into()));
        }
        let item_by_row = current
            .items
            .iter()
            .map(|item| (item.row_number, item))
            .collect::<HashMap<_, _>>();
        for selection in selections {
            let item = item_by_row.get(&selection.row_number).ok_or_else(|| {
                AppError::NotFound(format!("导入行不存在：{}", selection.row_number))
            })?;
            if selection.selected && !item.classification.can_apply() {
                return Err(AppError::Conflict(format!(
                    "第 {} 行当前分类不可应用",
                    selection.row_number
                )));
            }
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| AppError::database("开始更新导入选择", &error))?;
        for selection in selections {
            sqlx::query(
                "UPDATE local_aio_import_item SET selected = ? \
                 WHERE import_session_id = ? AND row_number = ?",
            )
            .bind(i64::from(selection.selected))
            .bind(session_id)
            .bind(i64::from(selection.row_number))
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("更新导入行选择", &error))?;
        }
        let mut next_items = current.items;
        for selection in selections {
            if let Some(item) = next_items
                .iter_mut()
                .find(|item| item.row_number == selection.row_number)
            {
                item.selected = selection.selected;
            }
        }
        let counts = ImportCounts::from_items(&next_items);
        let counts_json = serde_json::to_string(&counts)
            .map_err(|_| AppError::InvalidConfig("导入统计无法序列化".into()))?;
        sqlx::query(
            "UPDATE local_aio_import_session SET counts_json = ?, updated_at = ? WHERE id = ?",
        )
        .bind(counts_json)
        .bind(timestamp())
        .bind(session_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| AppError::database("更新导入会话统计", &error))?;
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交导入选择", &error))?;
        self.get(session_id).await
    }

    pub async fn mark_applied(&self, session_id: &str) -> AppResult<AioImportSession> {
        let result = sqlx::query(
            "UPDATE local_aio_import_session SET state = 'applied', updated_at = ? \
             WHERE id = ? AND state = 'preview'",
        )
        .bind(timestamp())
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::database("完成本地导入会话", &error))?;
        if result.rows_affected() != 1 {
            return Err(AppError::Conflict("导入会话已经结束或不存在".into()));
        }
        self.get(session_id).await
    }

    pub async fn discard(&self, session_id: &str) -> AppResult<()> {
        let result = sqlx::query(
            "UPDATE local_aio_import_session SET state = 'discarded', updated_at = ? \
             WHERE id = ? AND state = 'preview'",
        )
        .bind(timestamp())
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::database("放弃本地导入会话", &error))?;
        if result.rows_affected() != 1 {
            return Err(AppError::Conflict("导入会话已经结束或不存在".into()));
        }
        Ok(())
    }
}

fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 timestamp")
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use crate::domain::aio::inventory::{
        ImportClassification, InventoryValues, ReconciledImportItem,
    };
    use crate::formal::local_store::LocalStore;

    use super::AioImportRepository;
    use crate::domain::aio::assets::ImportSelection;

    async fn setup() -> (tempfile::TempDir, LocalStore, AioImportRepository) {
        let directory = tempdir().expect("temp dir");
        let store = LocalStore::open(directory.path().join("local.db"))
            .await
            .expect("local store");
        sqlx::query(
            "INSERT INTO local_project \
             (id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, \
              db_password_secret_ref, created_at, updated_at) \
             VALUES ('project-a', 'A', 'http://example.test', '127.0.0.1', 3306, 'test', \
                     'business', 'workbench', 'secret', '2026-08-27T00:00:00Z', '2026-08-27T00:00:00Z')",
        )
        .execute(store.pool())
        .await
        .expect("project");
        let repository = AioImportRepository::new(store.pool().clone());
        (directory, store, repository)
    }

    fn item(row: u32, classification: ImportClassification) -> ReconciledImportItem {
        ReconciledImportItem {
            row_number: row,
            values: InventoryValues {
                name: format!("node-{row}"),
                ip: format!("192.0.2.{row}"),
                mac: format!("0011223344{row:02}"),
                ..InventoryValues::default()
            },
            mac_normalized: Some(format!("0011223344{row:02}")),
            display_mac: None,
            selected: classification.can_apply(),
            classification,
            errors: Vec::new(),
            conflicts: Vec::new(),
            workbench_version: None,
            platform_aio_id: None,
            platform_fingerprint: None,
        }
    }

    #[tokio::test]
    async fn generated_location_is_visible_in_restored_preview_and_saved_asset() {
        use crate::domain::{aio::space::resolve_inventory_space, common::project_space::SpaceNode};
        use crate::infrastructure::local_sqlite::aio_node_repository::LocalAioRepository;
        let (_directory, store, repository) = setup().await;
        let spaces = [("1","项目","0","other"),("2","一号楼","1","building"),("3","二层","2","floor")]
            .into_iter().map(|(id,name,parent,kind)| SpaceNode {id:id.into(),name:name.into(),parent_id:Some(parent.into()),kind:kind.into()}).collect::<Vec<_>>();
        let mut row = item(2, ImportClassification::NewPending);
        row.values.space_path = Some("项目/一号楼/二层".into());
        resolve_inventory_space(&mut row.values, &spaces).unwrap();
        let session = repository.create_preview("project-a", "inventory.csv", "C:/inventory.csv", &[row]).await.unwrap();
        let restored = repository.get(&session.id).await.unwrap();
        assert_eq!(restored.items[0].values.addr_alias.as_deref(), Some("一号楼_二层"));
        assert_eq!(restored.items[0].values.location.as_deref(), Some("一号楼_二层"));
        let local = LocalAioRepository::new(store.pool().clone());
        local.apply_import("project-a", &session.id, &[(restored.items[0].values.clone(), None)]).await.unwrap();
        let saved = local.list("project-a").await.unwrap();
        assert_eq!(saved[0].building_id.as_deref(), Some("3"));
        assert_eq!(saved[0].addr_alias.as_deref(), Some("一号楼_二层"));
        assert_eq!(saved[0].location, saved[0].addr_alias);
        store.close().await;
    }

    #[tokio::test]
    async fn preview_is_local_restorable_selectable_and_sealed_after_apply() {
        let (_directory, store, repository) = setup().await;
        let session = repository
            .create_preview(
                "project-a",
                "inventory.csv",
                "C:/inventory.csv",
                &[
                    item(2, ImportClassification::NewPending),
                    item(3, ImportClassification::Conflict),
                ],
            )
            .await
            .expect("create preview");
        assert_eq!(session.counts.total, 2);
        assert_eq!(session.counts.selected, 1);
        let changed = repository
            .update_selection(
                &session.id,
                &[ImportSelection {
                    row_number: 2,
                    selected: false,
                }],
            )
            .await
            .expect("update selection");
        assert_eq!(changed.counts.selected, 0);
        let corrupt_selection = sqlx::query(
            "UPDATE local_aio_import_item SET selected = 2 \
             WHERE import_session_id = ? AND row_number = 2",
        )
        .bind(&session.id)
        .execute(store.pool())
        .await;
        assert!(corrupt_selection.is_err());
        assert!(
            !repository
                .get(&session.id)
                .await
                .expect("valid selection")
                .items[0]
                .selected
        );
        assert!(
            repository
                .update_selection(
                    &session.id,
                    &[ImportSelection {
                        row_number: 3,
                        selected: true,
                    }],
                )
                .await
                .is_err()
        );
        assert_eq!(
            repository
                .latest_open_for_project("project-a")
                .await
                .expect("latest")
                .expect("open preview")
                .id,
            session.id
        );
        repository
            .mark_applied(&session.id)
            .await
            .expect("mark applied");
        assert!(repository.update_selection(&session.id, &[]).await.is_err());
        assert!(
            repository
                .latest_open_for_project("project-a")
                .await
                .expect("latest")
                .is_none()
        );
        store.close().await;
    }

    #[tokio::test]
    async fn concurrent_preview_creation_keeps_one_open_session_per_project() {
        let (_directory, store, repository) = setup().await;
        let first = repository.clone();
        let second = repository.clone();
        let left_items = [item(2, ImportClassification::NewPending)];
        let right_items = [item(2, ImportClassification::NewPending)];
        let (left, right) = tokio::join!(
            first.create_preview("project-a", "left.csv", "C:/left.csv", &left_items,),
            second.create_preview("project-a", "right.csv", "C:/right.csv", &right_items,)
        );
        assert_ne!(left.is_ok(), right.is_ok());
        assert!(
            repository
                .latest_open_for_project("project-a")
                .await
                .expect("latest")
                .is_some()
        );
        store.close().await;
    }
}
