use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::model::*;
use crate::domain::smart_screen::rules::{validate_fields, validate_space};

const CONNECTION_REVISION_SQL: &str = "SELECT json_array(platform_url,db_host,db_port,db_user,db_tls_enabled,business_db,workbench_db,db_password_secret_ref) FROM local_project WHERE id=?";

#[derive(Clone)]
pub struct ScreenRepository {
    pool: SqlitePool,
}

fn encode<T: Serialize + ?Sized>(value: &T) -> AppResult<String> {
    serde_json::to_string(value).map_err(|_| AppError::InvalidConfig("智能屏数据无法保存".into()))
}
fn decode<T: DeserializeOwned>(value: &str) -> AppResult<T> {
    serde_json::from_str(value)
        .map_err(|_| AppError::Conflict("智能屏本机记录格式异常，请保留数据并检查".into()))
}
pub fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("valid timestamp")
}
fn db(error: sqlx::Error) -> AppError {
    if error
        .as_database_error()
        .is_some_and(|e| e.message().contains("SCREEN_ACTIVE_TASK"))
    {
        return AppError::Conflict("该屏仍有任务或待保存结果，不能修改或移除".into());
    }
    AppError::database("保存或读取本机智能屏数据", &error)
}

impl ScreenRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn require_project(&self, project: &str) -> AppResult<()> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_project WHERE id = ?")
            .bind(project)
            .fetch_one(&self.pool)
            .await
            .map_err(db)?;
        if count != 1 {
            return Err(AppError::NotFound("本机项目不存在".into()));
        }
        Ok(())
    }

    pub async fn scope(&self, project: &str) -> AppResult<Option<(String, String)>> {
        self.require_project(project).await?;
        let row = sqlx::query("SELECT business_project_id, data_source_id FROM local_screen_context WHERE local_project_id = ?")
            .bind(project).fetch_optional(&self.pool).await.map_err(db)?;
        Ok(row.and_then(|r| {
            Some((
                r.try_get::<Option<String>, _>("business_project_id")
                    .ok()??,
                r.try_get::<Option<String>, _>("data_source_id").ok()??,
            ))
        }))
    }

    /// 一个本机项目一旦关联平台资料，就不允许静默换到另一业务项目。
    pub async fn set_scope(&self, project: &str, business: &str, source: &str) -> AppResult<()> {
        self.require_project(project).await?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        Self::set_scope_in_transaction(&mut tx, project, business, source).await?;
        tx.commit().await.map_err(db)
    }

    async fn set_scope_in_transaction(
        tx: &mut Transaction<'_, Sqlite>,
        project: &str,
        business: &str,
        source: &str,
    ) -> AppResult<()> {
        if business.is_empty() || source.is_empty() {
            return Err(AppError::InvalidConfig("请选择业务项目".into()));
        }
        sqlx::query("INSERT INTO local_screen_context(local_project_id, business_project_id, data_source_id, updated_at) VALUES(?,?,?,?) ON CONFLICT(local_project_id) DO NOTHING")
            .bind(project).bind(business).bind(source).bind(now()).execute(&mut **tx).await.map_err(db)?;
        let row = sqlx::query("SELECT business_project_id, data_source_id FROM local_screen_context WHERE local_project_id = ?").bind(project).fetch_one(&mut **tx).await.map_err(db)?;
        let current: Option<String> = row.try_get("business_project_id").map_err(db)?;
        let data_source: Option<String> = row.try_get("data_source_id").map_err(db)?;
        if current.as_deref() != Some(business) || data_source.as_deref() != Some(source) {
            return Err(AppError::Conflict(
                "本机项目已关联另一平台范围，请新建本机项目，不能混用原资料和历史".into(),
            ));
        }
        Ok(())
    }

    /// 连接已改变但从未保存屏资料或历史时，允许重新识别平台；不清理任何业务数据。
    pub async fn clear_unused_scope(
        &self,
        project: &str,
        business: &str,
        source: &str,
    ) -> AppResult<()> {
        self.require_project(project).await?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        // 先取得写锁，并核对调用方看到的旧关联，避免并发刷新清掉新关联。
        let matched=sqlx::query("UPDATE local_screen_context SET updated_at=updated_at WHERE local_project_id=? AND business_project_id=? AND data_source_id=?")
            .bind(project).bind(business).bind(source).execute(&mut *tx).await.map_err(db)?;
        if matched.rows_affected() == 0 {
            return Ok(());
        }
        let used:i64=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM local_screen WHERE local_project_id=?1) OR EXISTS(SELECT 1 FROM local_screen_binding WHERE local_project_id=?1) OR EXISTS(SELECT 1 FROM local_screen_platform_cache WHERE local_project_id=?1) OR EXISTS(SELECT 1 FROM local_screen_draft WHERE local_project_id=?1) OR EXISTS(SELECT 1 FROM local_screen_observation WHERE local_project_id=?1) OR EXISTS(SELECT 1 FROM local_project_space_cache WHERE local_project_id=?1) OR EXISTS(SELECT 1 FROM local_screen_write_intent WHERE local_project_id=?1) OR EXISTS(SELECT 1 FROM local_screen_ignored_pair WHERE local_project_id=?1) OR EXISTS(SELECT 1 FROM local_screen_task_data WHERE local_project_id=?1) OR EXISTS(SELECT 1 FROM local_task t WHERE t.local_project_id=?1 AND (t.domain_type='smart_screen' OR t.state IN ('draft','checking','ready','queued','running','cancelling','finalizing_failed') OR EXISTS(SELECT 1 FROM local_task_result_guard g WHERE g.local_task_id=t.id)))")
            .bind(project).fetch_one(&mut *tx).await.map_err(db)?;
        if used != 0 {
            return Err(AppError::Conflict("该项目已有原平台的智能屏资料或操作记录，不能直接改用另一平台。请为新平台新建项目；原资料和历史已保留".into()));
        }
        sqlx::query("DELETE FROM local_screen_context WHERE local_project_id=? AND business_project_id=? AND data_source_id=?")
            .bind(project).bind(business).bind(source).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
    }

    pub async fn project_connection_revision(&self, project: &str) -> AppResult<String> {
        sqlx::query_scalar(CONNECTION_REVISION_SQL)
            .bind(project)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?
            .ok_or_else(|| AppError::NotFound("本机项目不存在".into()))
    }

    pub async fn require_idle(&self, project: &str, screen: &str) -> AppResult<()> {
        let count: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM local_task t WHERE t.local_project_id=? AND t.domain_type='smart_screen' AND (EXISTS(SELECT 1 FROM local_task_target r WHERE r.local_task_id=t.id AND r.resource_key=?) OR (t.operation_type='merge' AND EXISTS(SELECT 1 FROM local_screen_task_data d,json_each(d.plan_json,'$.detail.originalAssets') a WHERE d.local_task_id=t.id AND a.key=?))) AND (t.state IN ('draft','checking','ready','queued','running','cancelling','finalizing_failed') OR EXISTS(SELECT 1 FROM local_task_result_guard g WHERE g.local_task_id=t.id))) + (SELECT COUNT(*) FROM local_screen_write_intent WHERE local_project_id=? AND screen_id=? AND state IN ('prepared','submitted'))")
            .bind(project).bind(screen).bind(screen).bind(project).bind(screen).fetch_one(&self.pool).await.map_err(db)?;
        if count > 0 {
            return Err(AppError::Conflict(
                "该屏有未完成任务或待核实结果，请先处理".into(),
            ));
        }
        Ok(())
    }

    pub async fn save_local(
        &self,
        project: &str,
        fields: &ScreenFields,
        id: Option<&str>,
        expected_revision: Option<u64>,
    ) -> AppResult<String> {
        self.require_project(project).await?;
        validate_fields(fields, false)?;
        let (spaces, _, _) = self.spaces(project).await?;
        validate_space(fields, &spaces, false)?;
        let timestamp = now();
        let id = match id {
            Some(id) => {
                self.require_idle(project, id).await?;
                let expected = expected_revision.ok_or_else(|| {
                    AppError::Conflict("修改资料需要当前版本，请刷新后重试".into())
                })?;
                let changed = sqlx::query("UPDATE local_screen SET fields_json=?, revision=revision+1, updated_at=? WHERE local_project_id=? AND id=? AND removed=0 AND revision=? AND NOT EXISTS(SELECT 1 FROM local_screen_binding WHERE local_project_id=? AND local_screen_id=?)")
                    .bind(encode(fields)?).bind(&timestamp).bind(project).bind(id).bind(expected as i64).bind(project).bind(id).execute(&self.pool).await.map_err(db)?;
                if changed.rows_affected() != 1 {
                    return Err(AppError::Conflict(
                        "资料已修改、移出或注册，请刷新后重试".into(),
                    ));
                }
                id.to_string()
            }
            None => {
                let id = Uuid::now_v7().to_string();
                sqlx::query("INSERT INTO local_screen(local_project_id,id,fields_json,created_at,updated_at) VALUES(?,?,?,?,?)")
                    .bind(project).bind(&id).bind(encode(fields)?).bind(&timestamp).bind(&timestamp).execute(&self.pool).await.map_err(db)?;
                id
            }
        };
        Ok(id)
    }

    pub async fn import_local(
        &self,
        project: &str,
        inputs: &[ScreenFields],
    ) -> AppResult<Vec<String>> {
        self.require_project(project).await?;
        if inputs.is_empty() || inputs.len() > 10_000 {
            return Err(AppError::InvalidConfig("请选择 1 至 10000 行资料".into()));
        }
        let (spaces, _, _) = self.spaces(project).await?;
        for fields in inputs {
            validate_fields(fields, false)?;
            validate_space(fields, &spaces, false)?;
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        let mut ids = Vec::new();
        for fields in inputs {
            let id = Uuid::now_v7().to_string();
            sqlx::query("INSERT INTO local_screen(local_project_id,id,fields_json,created_at,updated_at) VALUES(?,?,?,?,?)")
                .bind(project).bind(&id).bind(encode(fields)?).bind(now()).bind(now()).execute(&mut *tx).await.map_err(db)?;
            ids.push(id);
        }
        tx.commit().await.map_err(db)?;
        Ok(ids)
    }

    pub async fn remove_local(&self, project: &str, id: &str) -> AppResult<()> {
        self.require_idle(project, id).await?;
        let changed = sqlx::query("UPDATE local_screen SET removed=1, revision=revision+1, updated_at=? WHERE local_project_id=? AND id=? AND removed=0 AND NOT EXISTS(SELECT 1 FROM local_screen_binding WHERE local_project_id=? AND local_screen_id=?)")
            .bind(now()).bind(project).bind(id).bind(project).bind(id).execute(&self.pool).await.map_err(db)?;
        if changed.rows_affected() != 1 {
            return Err(AppError::Conflict("只能移除本机未注册记录".into()));
        }
        Ok(())
    }

    pub async fn spaces(&self, project: &str) -> AppResult<(Vec<SpaceNode>, bool, Option<String>)> {
        let row=sqlx::query("SELECT nodes_json, complete, read_at FROM local_project_space_cache s JOIN local_screen_context c ON c.local_project_id=s.local_project_id AND c.business_project_id=s.business_project_id WHERE s.local_project_id=?")
            .bind(project).fetch_optional(&self.pool).await.map_err(db)?;
        match row {
            Some(row) => Ok((
                decode(&row.try_get::<String, _>("nodes_json").map_err(db)?)?,
                row.try_get::<i64, _>("complete").map_err(db)? == 1,
                Some(row.try_get("read_at").map_err(db)?),
            )),
            None => Ok((vec![], false, None)),
        }
    }

    /// 只接受一次完整读取；失败或部分读取不会替换缓存，也不会把缺失记录判为已删除。
    pub async fn replace_platform_cache(
        &self,
        project: &str,
        business: &str,
        assets: &[ScreenAsset],
        spaces: &[SpaceNode],
    ) -> AppResult<()> {
        let scope = self
            .scope(project)
            .await?
            .ok_or_else(|| AppError::Conflict("未选择业务项目".into()))?;
        if scope.0 != business {
            return Err(AppError::Conflict("业务项目已变化".into()));
        }
        let revision = self.project_connection_revision(project).await?;
        self.replace_platform_snapshot(project, business, &scope.1, &revision, assets, spaces)
            .await
    }

    /// 平台和空间完整读取后才建立关联，并与缓存一起提交；查询失败不留下空关联。
    pub async fn replace_platform_snapshot(
        &self,
        project: &str,
        business: &str,
        source: &str,
        expected_project_revision: &str,
        assets: &[ScreenAsset],
        spaces: &[SpaceNode],
    ) -> AppResult<()> {
        self.replace_snapshot_and_deleted(project, business, source, expected_project_revision, assets, spaces, &[]).await
    }

    pub async fn known_platform_ids(&self, project: &str, business: &str) -> AppResult<Vec<String>> {
        sqlx::query_scalar("SELECT platform_screen_id FROM local_screen_platform_cache WHERE local_project_id=?1 AND business_project_id=?2 UNION SELECT platform_screen_id FROM local_screen_binding WHERE local_project_id=?1 AND business_project_id=?2 UNION SELECT platform_screen_id FROM local_screen_draft WHERE local_project_id=?1 AND business_project_id=?2")
            .bind(project).bind(business).fetch_all(&self.pool).await.map_err(db)
    }

    pub async fn replace_snapshot_and_deleted(
        &self, project: &str, business: &str, source: &str,
        expected_project_revision: &str, assets: &[ScreenAsset], spaces: &[SpaceNode], deleted: &[String],
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        Self::set_scope_in_transaction(&mut tx, project, business, source).await?;
        let current: String = sqlx::query_scalar(CONNECTION_REVISION_SQL)
            .bind(project)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
        if current != expected_project_revision {
            return Err(AppError::Conflict(
                "项目连接在读取期间已改变，请刷新列表".into(),
            ));
        }
        for id in deleted {
            if assets.iter().any(|a| &a.id == id) {
                return Err(AppError::Conflict("平台读取结果包含矛盾的删除记录".into()));
            }
            // 操作、日志和未确认写入的依据独立保存，不随资产删除。
            sqlx::query("DELETE FROM local_screen_observation WHERE local_project_id=?1 AND (screen_id=?2 OR screen_id IN (SELECT local_screen_id FROM local_screen_binding WHERE local_project_id=?1 AND business_project_id=?3 AND platform_screen_id=?2))")
                .bind(project).bind(id).bind(business).execute(&mut *tx).await.map_err(db)?;
            sqlx::query("DELETE FROM local_screen WHERE local_project_id=?1 AND id IN (SELECT local_screen_id FROM local_screen_binding WHERE local_project_id=?1 AND business_project_id=?2 AND platform_screen_id=?3)")
                .bind(project).bind(business).bind(id).execute(&mut *tx).await.map_err(db)?;
            sqlx::query("DELETE FROM local_screen_draft WHERE local_project_id=? AND business_project_id=? AND platform_screen_id=?")
                .bind(project).bind(business).bind(id).execute(&mut *tx).await.map_err(db)?;
        }
        let batch = Uuid::now_v7().to_string();
        let timestamp = now();
        for asset in assets {
            let mut raw = asset.clone();
            raw.revision = 0;
            raw.aliases.clear();
            let json = encode(&raw)?;
            sqlx::query("INSERT INTO local_screen_platform_cache(local_project_id,business_project_id,platform_screen_id,asset_json,read_batch,read_at) VALUES(?,?,?,?,?,?) ON CONFLICT(local_project_id,business_project_id,platform_screen_id) DO UPDATE SET revision=CASE WHEN asset_json=excluded.asset_json THEN revision ELSE revision+1 END, asset_json=excluded.asset_json, read_batch=excluded.read_batch, read_at=excluded.read_at")
                .bind(project).bind(business).bind(&asset.id).bind(json).bind(&batch).bind(&timestamp).execute(&mut *tx).await.map_err(db)?;
        }
        sqlx::query("DELETE FROM local_screen_platform_cache WHERE local_project_id=? AND business_project_id=? AND read_batch<>?")
            .bind(project).bind(business).bind(&batch).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("INSERT INTO local_project_space_cache(local_project_id,business_project_id,nodes_json,complete,read_at) VALUES(?,?,?,1,?) ON CONFLICT(local_project_id,business_project_id) DO UPDATE SET nodes_json=excluded.nodes_json,complete=1,read_at=excluded.read_at")
            .bind(project).bind(business).bind(encode(spaces)?).bind(&timestamp).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
    }

    pub async fn snapshot(&self, project: &str) -> AppResult<ScreenSnapshot> {
        self.require_project(project).await?;
        let mut snapshot = ScreenSnapshot::default();
        let (spaces, complete, read_at) = self.spaces(project).await?;
        snapshot.spaces = spaces;
        snapshot.spaces_available = complete;
        snapshot.platform_read_at = read_at;
        let rows=sqlx::query("SELECT id,fields_json,revision FROM local_screen s WHERE local_project_id=? AND removed=0 AND NOT EXISTS(SELECT 1 FROM local_screen_binding b WHERE b.local_project_id=s.local_project_id AND b.local_screen_id=s.id) ORDER BY created_at,id")
            .bind(project).fetch_all(&self.pool).await.map_err(db)?;
        for row in rows {
            snapshot.screens.push(ScreenAsset {
                id: row.try_get("id").map_err(db)?,
                source: "local".into(),
                fields: decode(&row.try_get::<String, _>("fields_json").map_err(db)?)?,
                revision: row.try_get::<i64, _>("revision").map_err(db)? as u64,
                platform_status: "unknown".into(),
                ..Default::default()
            });
        }
        if let Some((business, _)) = self.scope(project).await? {
            snapshot.business_project_id = Some(business.clone());
            let rows=sqlx::query("SELECT platform_screen_id,asset_json,revision FROM local_screen_platform_cache WHERE local_project_id=? AND business_project_id=? ORDER BY platform_screen_id")
                .bind(project).bind(&business).fetch_all(&self.pool).await.map_err(db)?;
            let aliases=sqlx::query("SELECT local_screen_id,platform_screen_id FROM local_screen_binding WHERE local_project_id=? AND business_project_id=?")
                .bind(project).bind(&business).fetch_all(&self.pool).await.map_err(db)?;
            for row in rows {
                let mut asset: ScreenAsset =
                    decode(&row.try_get::<String, _>("asset_json").map_err(db)?)?;
                asset.revision = row.try_get::<i64, _>("revision").map_err(db)? as u64;
                asset.aliases = aliases
                    .iter()
                    .filter(|r| r.get::<String, _>("platform_screen_id") == asset.id)
                    .map(|r| r.get("local_screen_id"))
                    .collect();
                snapshot.screens.push(asset);
            }
            for row in sqlx::query("SELECT * FROM local_screen_draft WHERE local_project_id=? AND business_project_id=?").bind(project).bind(&business).fetch_all(&self.pool).await.map_err(db)? {
                let id:String=row.try_get("platform_screen_id").map_err(db)?;
                snapshot.platform_drafts.insert(id.clone(),ScreenDraft {screen_id:id,base_revision:row.try_get::<i64,_>("base_revision").map_err(db)? as u64,revision:row.try_get::<i64,_>("revision").map_err(db)? as u64,base:decode(&row.try_get::<String,_>("base_json").map_err(db)?)?,values:decode(&row.try_get::<String,_>("values_json").map_err(db)?)?,updated_at:row.try_get("updated_at").map_err(db)?});
            }
        }
        for row in sqlx::query("SELECT result_json FROM local_screen_observation WHERE local_project_id=? ORDER BY observed_at DESC,id DESC").bind(project).fetch_all(&self.pool).await.map_err(db)? {
            let item:ScreenObservation=decode(&row.try_get::<String,_>("result_json").map_err(db)?)?;
            snapshot.observations.entry(item.screen_id.clone()).or_default().push(item);
        }
        snapshot.ignored_pairs = sqlx::query_scalar(
            "SELECT pair_key FROM local_screen_ignored_pair WHERE local_project_id=?",
        )
        .bind(project)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(snapshot)
    }

    pub async fn asset(&self, project: &str, id: &str) -> AppResult<ScreenAsset> {
        self.snapshot(project)
            .await?
            .screens
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| AppError::NotFound("该屏不在当前项目中".into()))
    }

    pub async fn save_draft(
        &self,
        project: &str,
        id: &str,
        values: &ScreenFields,
        expected_revision: Option<u64>,
    ) -> AppResult<()> {
        validate_fields(values, false)?;
        let scope = self
            .scope(project)
            .await?
            .ok_or_else(|| AppError::Conflict("未选择业务项目".into()))?;
        let asset = self.asset(project, id).await?;
        if asset.source != "platform" {
            return Err(AppError::InvalidConfig("仅已注册屏保存平台资料草稿".into()));
        }
        let (spaces, _, _) = self.spaces(project).await?;
        validate_space(values, &spaces, false)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        // 第一次编辑保留平台基准，之后只更新草稿，不重设比较基准。
        let changed=sqlx::query("INSERT INTO local_screen_draft(local_project_id,platform_screen_id,business_project_id,base_json,values_json,base_revision,revision,updated_at) SELECT ?,?,?,?,?,?,1,? WHERE ?=0 ON CONFLICT(local_project_id,business_project_id,platform_screen_id) DO NOTHING")
            .bind(project).bind(id).bind(&scope.0).bind(encode(&asset.fields)?).bind(encode(values)?).bind(asset.revision as i64).bind(now()).bind(expected_revision.unwrap_or(0) as i64).execute(&mut *tx).await.map_err(db)?;
        if changed.rows_affected() == 0 {
            let changed=sqlx::query("UPDATE local_screen_draft SET values_json=?,revision=revision+1,updated_at=? WHERE local_project_id=? AND business_project_id=? AND platform_screen_id=? AND revision=?")
                .bind(encode(values)?).bind(now()).bind(project).bind(&scope.0).bind(id).bind(expected_revision.unwrap_or(0) as i64).execute(&mut *tx).await.map_err(db)?;
            if changed.rows_affected() != 1 {
                return Err(AppError::Conflict("草稿已被更新，请重新打开资料".into()));
            }
        }
        tx.commit().await.map_err(db)
    }

    /// 人员重新打开资料并确认保存时，以所见平台资料重新建立基准。
    pub async fn save_draft_checked(
        &self,
        project: &str,
        id: &str,
        values: &ScreenFields,
        asset_revision: u64,
        draft_revision: u64,
    ) -> AppResult<()> {
        validate_fields(values, false)?;
        let (spaces, _, _) = self.spaces(project).await?;
        validate_space(values, &spaces, false)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query(
            "UPDATE local_screen_context SET updated_at=updated_at WHERE local_project_id=?",
        )
        .bind(project)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        let row=sqlx::query("SELECT c.business_project_id,c.asset_json,c.revision FROM local_screen_platform_cache c JOIN local_screen_context s ON s.local_project_id=c.local_project_id AND s.business_project_id=c.business_project_id WHERE c.local_project_id=? AND c.platform_screen_id=?")
            .bind(project).bind(id).fetch_optional(&mut *tx).await.map_err(db)?.ok_or_else(||AppError::NotFound("平台缓存不存在，请重新读取".into()))?;
        if row.get::<i64, _>("revision") as u64 != asset_revision {
            return Err(AppError::Conflict("平台资料已变化，请重新打开编辑".into()));
        }
        let business: String = row.try_get("business_project_id").map_err(db)?;
        let asset: ScreenAsset = decode(&row.try_get::<String, _>("asset_json").map_err(db)?)?;
        let stored:Option<i64>=sqlx::query_scalar("SELECT revision FROM local_screen_draft WHERE local_project_id=? AND business_project_id=? AND platform_screen_id=?").bind(project).bind(&business).bind(id).fetch_optional(&mut *tx).await.map_err(db)?;
        if stored.unwrap_or(0) as u64 != draft_revision {
            return Err(AppError::Conflict("草稿已变化，请重新打开编辑".into()));
        }
        sqlx::query("INSERT INTO local_screen_draft(local_project_id,platform_screen_id,business_project_id,base_json,values_json,base_revision,revision,updated_at) VALUES(?,?,?,?,?,?,1,?) ON CONFLICT(local_project_id,business_project_id,platform_screen_id) DO UPDATE SET base_json=excluded.base_json,values_json=excluded.values_json,base_revision=excluded.base_revision,revision=revision+1,updated_at=excluded.updated_at")
            .bind(project).bind(id).bind(business).bind(encode(&asset.fields)?).bind(encode(values)?).bind(asset_revision as i64).bind(now()).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
    }

    pub async fn discard_draft(
        &self,
        project: &str,
        id: &str,
        expected_revision: u64,
    ) -> AppResult<()> {
        let changed=sqlx::query("DELETE FROM local_screen_draft WHERE local_project_id=? AND platform_screen_id=? AND revision=?")
            .bind(project).bind(id).bind(expected_revision as i64).execute(&self.pool).await.map_err(db)?;
        if changed.rows_affected() != 1 {
            return Err(AppError::Conflict("草稿已变化，请刷新后再放弃".into()));
        }
        Ok(())
    }

    pub async fn append_observation(
        &self,
        project: &str,
        item: &ScreenObservation,
    ) -> AppResult<()> {
        self.require_project(project).await?;
        let json = encode(item)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("INSERT INTO local_screen_observation(id,local_project_id,screen_id,operation_type,observed_ip,observed_at,task_id,result_json) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(id) DO NOTHING")
            .bind(&item.id).bind(project).bind(&item.screen_id).bind(&item.operation_type).bind(&item.observed_ip).bind(&item.observed_at).bind(&item.task_id).bind(&json).execute(&mut *tx).await.map_err(db)?;
        let stored: (String, String) = sqlx::query_as(
            "SELECT local_project_id,result_json FROM local_screen_observation WHERE id=?",
        )
        .bind(&item.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if stored.0 != project || stored.1 != json {
            return Err(AppError::Conflict("不能用另一检查结果覆盖已有实测".into()));
        }
        tx.commit().await.map_err(db)
    }

    pub async fn prepare_intent(&self, project: &str, intent: &WriteIntent) -> AppResult<()> {
        if intent.state != "prepared" {
            return Err(AppError::InvalidConfig("新请求必须处于准备状态".into()));
        }
        let scope = self
            .scope(project)
            .await?
            .ok_or_else(|| AppError::Conflict("未选择业务项目".into()))?;
        if scope.0 != intent.business_project_id {
            return Err(AppError::Conflict("请求所属业务项目不符".into()));
        }
        let payload = encode(&intent.payload)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("INSERT INTO local_screen_write_intent(request_id,local_project_id,business_project_id,screen_id,platform_screen_id,operation_type,payload_json,state,created_at,updated_at) VALUES(?,?,?,?,?,?,?,'prepared',?,?) ON CONFLICT(request_id) DO NOTHING")
            .bind(&intent.request_id).bind(project).bind(&intent.business_project_id).bind(&intent.screen_id).bind(&intent.platform_screen_id).bind(&intent.operation_type).bind(&payload).bind(now()).bind(now()).execute(&mut *tx).await.map_err(db)?;
        let row = sqlx::query("SELECT * FROM local_screen_write_intent WHERE request_id=?")
            .bind(&intent.request_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(db)?;
        if row.get::<String, _>("local_project_id") != project
            || row.get::<String, _>("screen_id") != intent.screen_id
            || row.get::<String, _>("platform_screen_id") != intent.platform_screen_id
            || row.get::<String, _>("business_project_id") != intent.business_project_id
            || row.get::<String, _>("operation_type") != intent.operation_type
            || row.get::<String, _>("payload_json") != payload
        {
            return Err(AppError::Conflict(
                "同一请求号不能用于另一目标或不同内容".into(),
            ));
        }
        tx.commit().await.map_err(db)
    }

    pub async fn intents(&self, project: &str) -> AppResult<Vec<WriteIntent>> {
        let rows = sqlx::query(
            "SELECT * FROM local_screen_write_intent WHERE local_project_id=? ORDER BY created_at",
        )
        .bind(project)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(WriteIntent {
                    request_id: r.try_get("request_id").map_err(db)?,
                    business_project_id: r.try_get("business_project_id").map_err(db)?,
                    screen_id: r.try_get("screen_id").map_err(db)?,
                    platform_screen_id: r.try_get("platform_screen_id").map_err(db)?,
                    operation_type: r.try_get("operation_type").map_err(db)?,
                    payload: decode(&r.try_get::<String, _>("payload_json").map_err(db)?)?,
                    state: r.try_get("state").map_err(db)?,
                    result: r
                        .try_get::<Option<String>, _>("result_json")
                        .map_err(db)?
                        .map(|s| decode(&s))
                        .transpose()?,
                })
            })
            .collect()
    }

    pub async fn update_intent(
        &self,
        project: &str,
        id: &str,
        expected: &str,
        next: &str,
        result: Option<&serde_json::Value>,
    ) -> AppResult<()> {
        if !matches!(
            (expected, next),
            ("prepared", "submitted")
                | ("prepared", "not_applied")
                | ("submitted", "confirmed")
                | ("submitted", "conflict")
                | ("submitted", "not_applied")
        ) {
            return Err(AppError::InvalidConfig("不允许跳过平台写入结果核实".into()));
        }
        let changed=sqlx::query("UPDATE local_screen_write_intent SET state=?,result_json=?,updated_at=? WHERE local_project_id=? AND request_id=? AND state=?")
            .bind(next).bind(result.map(encode).transpose()?).bind(now()).bind(project).bind(id).bind(expected).execute(&self.pool).await.map_err(db)?;
        if changed.rows_affected() != 1 {
            return Err(AppError::Conflict(
                "平台写入请求状态已变化，请先核实结果".into(),
            ));
        }
        Ok(())
    }

    /// 平台已确认后，本机关联、缓存、草稿处理和请求确认一起提交。
    pub async fn confirm_registration(
        &self,
        project: &str,
        business: &str,
        original_id: &str,
        was_local: bool,
        request_id: &str,
        asset: &ScreenAsset,
        submitted: &ScreenFields,
        submitted_draft_revision: u64,
        evidence: &serde_json::Value,
    ) -> AppResult<()> {
        use crate::domain::smart_screen::registration::{field_values, set_field};
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("UPDATE local_screen_context SET updated_at=updated_at WHERE local_project_id=? AND business_project_id=?")
            .bind(project).bind(business).execute(&mut *tx).await.map_err(db)?;
        let intent=sqlx::query("SELECT state,platform_screen_id FROM local_screen_write_intent WHERE local_project_id=? AND business_project_id=? AND request_id=? AND screen_id=?")
            .bind(project).bind(business).bind(request_id).bind(original_id).fetch_optional(&mut *tx).await.map_err(db)?
            .ok_or_else(||AppError::Conflict("原平台写入请求不存在，不能直接关联".into()))?;
        if intent.get::<String, _>("platform_screen_id") != asset.id || asset.source != "platform" {
            return Err(AppError::Conflict("平台确认记录与原请求不一致".into()));
        }
        let status: String = intent.try_get("state").map_err(db)?;
        if status == "confirmed" {
            return Ok(());
        }
        if status != "submitted" {
            return Err(AppError::Conflict("请求未进入平台提交阶段".into()));
        }
        if was_local {
            let removed: Option<i64> = sqlx::query_scalar(
                "SELECT removed FROM local_screen WHERE local_project_id=? AND id=?",
            )
            .bind(project)
            .bind(original_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db)?;
            if removed != Some(0) {
                return Err(AppError::Conflict(
                    "原本机记录已移出，请核实本机关联".into(),
                ));
            }
            sqlx::query("INSERT INTO local_screen_binding(local_project_id,local_screen_id,business_project_id,platform_screen_id,request_id,evidence_json,created_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(local_project_id,local_screen_id) DO NOTHING")
                .bind(project).bind(original_id).bind(business).bind(&asset.id).bind(request_id).bind(encode(evidence)?).bind(now()).execute(&mut *tx).await.map_err(db)?;
            let bound:(String,String)=sqlx::query_as("SELECT business_project_id,platform_screen_id FROM local_screen_binding WHERE local_project_id=? AND local_screen_id=?")
                .bind(project).bind(original_id).fetch_one(&mut *tx).await.map_err(db)?;
            if bound != (business.into(), asset.id.clone()) {
                return Err(AppError::Conflict("原本机记录已经关联另一平台屏".into()));
            }
        }
        let mut raw = asset.clone();
        raw.revision = 0;
        raw.aliases.clear();
        sqlx::query("INSERT INTO local_screen_platform_cache(local_project_id,business_project_id,platform_screen_id,asset_json,read_batch,read_at) VALUES(?,?,?,?,?,?) ON CONFLICT(local_project_id,business_project_id,platform_screen_id) DO UPDATE SET revision=CASE WHEN asset_json=excluded.asset_json THEN revision ELSE revision+1 END,asset_json=excluded.asset_json,read_batch=excluded.read_batch,read_at=excluded.read_at")
            .bind(project).bind(business).bind(&asset.id).bind(encode(&raw)?).bind(request_id).bind(now()).execute(&mut *tx).await.map_err(db)?;
        if !was_local {
            if let Some(draft)=sqlx::query("SELECT values_json,revision FROM local_screen_draft WHERE local_project_id=? AND business_project_id=? AND platform_screen_id=?")
                .bind(project).bind(business).bind(&asset.id).fetch_optional(&mut *tx).await.map_err(db)? {
                let revision:i64=draft.try_get("revision").map_err(db)?;
                let mut values:ScreenFields=decode(&draft.try_get::<String,_>("values_json").map_err(db)?)?;
                // 显式改回旧值也是新修改；不能只按“等于旧基准”就将它清除。
                if revision as u64>submitted_draft_revision {
                    let old_values=field_values(&values);let sent=field_values(submitted);let saved=field_values(&asset.fields);
                    for(key,value)in old_values {if value==sent[key]{set_field(&mut values,key,saved[key].clone());}}
                } else {values=asset.fields.clone();}
                if values==asset.fields {
                    sqlx::query("DELETE FROM local_screen_draft WHERE local_project_id=? AND business_project_id=? AND platform_screen_id=?")
                        .bind(project).bind(business).bind(&asset.id).execute(&mut *tx).await.map_err(db)?;
                }else{
                    let cache_revision:i64=sqlx::query_scalar("SELECT revision FROM local_screen_platform_cache WHERE local_project_id=? AND business_project_id=? AND platform_screen_id=?")
                        .bind(project).bind(business).bind(&asset.id).fetch_one(&mut *tx).await.map_err(db)?;
                    sqlx::query("UPDATE local_screen_draft SET base_json=?,values_json=?,base_revision=?,revision=revision+1,updated_at=? WHERE local_project_id=? AND business_project_id=? AND platform_screen_id=?")
                        .bind(encode(&asset.fields)?).bind(encode(&values)?).bind(cache_revision).bind(now()).bind(project).bind(business).bind(&asset.id).execute(&mut *tx).await.map_err(db)?;
                }
            }
        }
        sqlx::query("UPDATE local_screen_write_intent SET state='confirmed',result_json=?,updated_at=? WHERE request_id=? AND local_project_id=? AND state='submitted'")
            .bind(encode(evidence)?).bind(now()).bind(request_id).bind(project).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
    }

    pub async fn ignore_pair(&self, project: &str, key: &str) -> AppResult<()> {
        self.require_project(project).await?;
        sqlx::query("INSERT INTO local_screen_ignored_pair(local_project_id,pair_key,created_at) VALUES(?,?,?) ON CONFLICT DO NOTHING").bind(project).bind(key).bind(now()).execute(&self.pool).await.map_err(db)?;
        Ok(())
    }

    /// 状态和版本回读只刷新平台缓存，不更改资料草稿或本机关联。
    pub async fn confirm_business_value(
        &self,
        project: &str,
        business: &str,
        request: &str,
        asset: &ScreenAsset,
        evidence: &serde_json::Value,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let row=sqlx::query("SELECT state,platform_screen_id FROM local_screen_write_intent WHERE local_project_id=? AND business_project_id=? AND request_id=?")
            .bind(project).bind(business).bind(request).fetch_optional(&mut *tx).await.map_err(db)?.ok_or_else(||AppError::Conflict("原业务更新请求不存在".into()))?;
        if row.get::<String, _>("platform_screen_id") != asset.id {
            return Err(AppError::Conflict("确认结果不属于原屏记录".into()));
        }
        let status: String = row.try_get("state").map_err(db)?;
        if status == "confirmed" {
            return Ok(());
        }
        if status != "submitted" {
            return Err(AppError::Conflict("业务更新请求状态不正确".into()));
        }
        let mut raw = asset.clone();
        raw.revision = 0;
        raw.aliases.clear();
        sqlx::query("INSERT INTO local_screen_platform_cache(local_project_id,business_project_id,platform_screen_id,asset_json,read_batch,read_at) VALUES(?,?,?,?,?,?) ON CONFLICT(local_project_id,business_project_id,platform_screen_id) DO UPDATE SET revision=CASE WHEN asset_json=excluded.asset_json THEN revision ELSE revision+1 END,asset_json=excluded.asset_json,read_batch=excluded.read_batch,read_at=excluded.read_at")
            .bind(project).bind(business).bind(&asset.id).bind(encode(&raw)?).bind(request).bind(now()).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("UPDATE local_screen_write_intent SET state='confirmed',result_json=?,updated_at=? WHERE local_project_id=? AND request_id=? AND state='submitted'")
            .bind(encode(evidence)?).bind(now()).bind(project).bind(request).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
    }
}
