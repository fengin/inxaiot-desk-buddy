use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::inventory::{ImportCounts, ReconciledImportItem};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AioImportSession {
    pub id: String,
    pub local_project_id: String,
    pub file_name: String,
    pub file_path: String,
    pub state: String,
    pub counts: ImportCounts,
    pub created_at: String,
    pub updated_at: String,
    pub items: Vec<ReconciledImportItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSelection {
    pub row_number: u32,
    pub selected: bool,
}

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
            item.selected = row.try_get::<i64, _>("selected").unwrap_or_default() != 0;
            items.push(item);
        }
        let counts_json: String = session
            .try_get("counts_json")
            .map_err(|error| AppError::database("解析本地导入统计", &error))?;
        Ok(AioImportSession {
            id: session.try_get("id").unwrap_or_default(),
            local_project_id: session.try_get("local_project_id").unwrap_or_default(),
            file_name: session.try_get("file_name").unwrap_or_default(),
            file_path: session.try_get("file_path").unwrap_or_default(),
            state: session.try_get("state").unwrap_or_default(),
            counts: serde_json::from_str(&counts_json)
                .map_err(|_| AppError::InvalidConfig("本地导入统计已损坏".into()))?,
            created_at: session.try_get("created_at").unwrap_or_default(),
            updated_at: session.try_get("updated_at").unwrap_or_default(),
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

    use super::{AioImportRepository, ImportSelection};

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
