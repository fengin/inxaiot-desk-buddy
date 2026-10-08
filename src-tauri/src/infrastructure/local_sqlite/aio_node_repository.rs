use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::inventory::{InventoryValues, WorkbenchNodeSnapshot};
use crate::domain::aio::mac::MacAddress;

#[derive(Clone)]
pub struct LocalAioRepository { pool: SqlitePool }

impl LocalAioRepository {
    pub fn new(pool: SqlitePool) -> Self { Self { pool } }

    pub async fn list(&self, project: &str) -> AppResult<Vec<WorkbenchNodeSnapshot>> {
        let rows = sqlx::query("SELECT mac_normalized,values_json,version FROM local_aio_node WHERE local_project_id=? ORDER BY mac_normalized")
            .bind(project).fetch_all(&self.pool).await.map_err(db)?;
        rows.into_iter().map(|row| {
            let text: String = row.try_get("values_json").map_err(db)?;
            let values: InventoryValues = serde_json::from_str(&text).map_err(|_| AppError::InvalidConfig("本机一体机资料格式无效".into()))?;
            let version: i64 = row.try_get("version").map_err(db)?;
            Ok(WorkbenchNodeSnapshot {
                mac_normalized: row.try_get("mac_normalized").map_err(db)?, name: values.name, ip: values.ip,
                building_id: values.building_id, region_id: values.region_id, addr_alias: values.addr_alias,
                floor: values.floor, location: values.location, remark: values.remark,
                platform_aio_id: None, management_state: "pending".into(), source: "local".into(),
                last_operation_id: None, version: version as u64,
            })
        }).collect()
    }

    pub async fn save_many(&self, project: &str, records: &[(InventoryValues, Option<u64>)]) -> AppResult<()> {
        self.save(project, None, records).await
    }

    pub async fn apply_import(&self, project: &str, session: &str, records: &[(InventoryValues, Option<u64>)]) -> AppResult<()> {
        self.save(project, Some(session), records).await
    }

    async fn save(&self, project: &str, session: Option<&str>, records: &[(InventoryValues, Option<u64>)]) -> AppResult<()> {
        if project.is_empty() || records.is_empty() { return Err(AppError::InvalidConfig("项目和一体机资料不能为空".into())); }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await.map_err(db)?;
        if let Some(session) = session {
            let updated = sqlx::query("UPDATE local_aio_import_session SET state='applied',updated_at=? WHERE id=? AND local_project_id=? AND state='preview'")
                .bind(time::OffsetDateTime::now_utc().unix_timestamp().to_string()).bind(session).bind(project).execute(&mut *tx).await.map_err(db)?;
            if updated.rows_affected() != 1 { return Err(AppError::Conflict("导入预览已结束或不属于当前项目".into())); }
        }
        for (values, expected) in records {
            let mac = MacAddress::parse(&values.mac)?.normalized().to_string();
            ensure_idle(&mut tx, project, &mac).await?;
            let duplicate: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_aio_node WHERE local_project_id=? AND mac_normalized<>? AND json_extract(values_json,'$.ip')=?")
                .bind(project).bind(&mac).bind(&values.ip).fetch_one(&mut *tx).await.map_err(db)?;
            if duplicate > 0 { return Err(AppError::Conflict("IP 已被本机清单中的另一台一体机使用".into())); }
            let json = serde_json::to_string(values).map_err(|_| AppError::InvalidConfig("一体机资料无法保存".into()))?;
            let now = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
            let affected = if let Some(version) = expected {
                sqlx::query("UPDATE local_aio_node SET values_json=?,version=version+1,updated_at=? WHERE local_project_id=? AND mac_normalized=? AND version=?")
                    .bind(&json).bind(&now).bind(project).bind(&mac).bind(*version as i64).execute(&mut *tx).await.map_err(db)?.rows_affected()
            } else {
                sqlx::query("INSERT INTO local_aio_node(local_project_id,mac_normalized,values_json,created_at,updated_at) VALUES(?,?,?,?,?) ON CONFLICT(local_project_id,mac_normalized) DO NOTHING")
                    .bind(project).bind(&mac).bind(&json).bind(&now).bind(&now).execute(&mut *tx).await.map_err(db)?.rows_affected()
            };
            if affected != 1 { return Err(AppError::Conflict("本机一体机资料已变化，请刷新后重试".into())); }
        }
        tx.commit().await.map_err(db)
    }

    /// 平台已确认登记后退出本机待实施列表；删除受版本保护，不移除其他项目资料。
    pub async fn retire(&self, project: &str, mac: &str, version: u64) -> AppResult<()> {
        sqlx::query("DELETE FROM local_aio_node WHERE local_project_id=? AND mac_normalized=? AND version=?")
            .bind(project).bind(mac).bind(version as i64).execute(&self.pool).await.map_err(db)?;
        Ok(())
    }
}

async fn ensure_idle(tx: &mut Transaction<'_, Sqlite>, project: &str, mac: &str) -> AppResult<()> {
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_task t JOIN local_task_target r ON r.local_task_id=t.id WHERE t.local_project_id=? AND t.domain_type='aio' AND r.resource_type='aio' AND r.resource_key=? AND (t.state IN ('queued','running','cancelling','finalizing_failed') OR EXISTS(SELECT 1 FROM local_task_result_guard g WHERE g.local_task_id=t.id))")
        .bind(project).bind(mac).fetch_one(&mut **tx).await.map_err(db)?;
    if active > 0 { return Err(AppError::Conflict("该一体机有排队、执行中或结果待核实的任务，请处理完成后再修改".into())); }
    Ok(())
}

fn db(error: sqlx::Error) -> AppError { AppError::database("保存或读取本机待实施一体机", &error) }
