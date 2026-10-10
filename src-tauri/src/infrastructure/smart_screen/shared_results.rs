use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::model::ScreenTargetResult;
use crate::formal::operation_repository::validate_summary_json;
use crate::infrastructure::project_context::map_formal_error;
use sqlx::{MySqlPool, Row};

/// 屏业务复用公共记录表。请求号由本机任务预先生成，断线补存不生成新操作。
#[derive(Clone)]
pub struct ScreenSharedResults {
    pool: MySqlPool,
}

pub struct SharedScreenOperation<'a> {
    pub id: &'a str,
    pub business_project_id: &'a str,
    pub action: &'a str,
    pub name: &'a str,
    pub operator: &'a str,
    pub instance_id: &'a str,
    pub targets: &'a [String],
    pub started_at: Option<&'a str>,
}

fn db(error: sqlx::Error) -> AppError {
    tracing::error!(error=?crate::core::log_safety::safe_error(&error),"smart screen shared result failure");
    AppError::database("保存智能屏共享结果", &error)
}

impl ScreenSharedResults {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
    pub async fn bind_source(&self, source: &str, schema: &str) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("INSERT INTO workbench_data_source(singleton_id,data_source_id,platform_schema,created_at) VALUES(1,?,?,UTC_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE singleton_id=singleton_id")
            .bind(source).bind(schema).execute(&mut *tx).await.map_err(db)?;
        let row=sqlx::query("SELECT data_source_id,platform_schema FROM workbench_data_source WHERE singleton_id=1 FOR UPDATE").fetch_one(&mut *tx).await.map_err(db)?;
        if row.get::<String, _>("data_source_id") != source
            || row.get::<String, _>("platform_schema") != schema
        {
            return Err(AppError::Conflict(
                "工作台专用库已属于另一平台数据源，不能混用操作记录".into(),
            ));
        }
        tx.commit().await.map_err(db)
    }

    pub async fn start(&self, op: &SharedScreenOperation<'_>) -> AppResult<()> {
        if uuid::Uuid::parse_str(op.id).is_err()
            || op.business_project_id.is_empty()
            || op.targets.is_empty()
            || op.operator.is_empty()
        {
            return Err(AppError::InvalidConfig(
                "共享操作缺少请求号、业务项目、操作人或注册屏".into(),
            ));
        }
        let targets: std::collections::BTreeSet<_> = op.targets.iter().collect();
        if targets.len() != op.targets.len() {
            return Err(AppError::InvalidConfig("共享操作包含重复目标".into()));
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        let started = op
            .started_at
            .map(|value| {
                if let Ok(nanos) = value.parse::<i128>() {
                    time::OffsetDateTime::from_unix_timestamp_nanos(nanos)
                        .map_err(|_| AppError::InvalidConfig("原任务开始时间无效".into()))
                } else {
                    time::OffsetDateTime::parse(
                        value,
                        &time::format_description::well_known::Rfc3339,
                    )
                    .map_err(|_| AppError::InvalidConfig("原任务开始时间无效".into()))
                }
            })
            .transpose()?
            .unwrap_or_else(time::OffsetDateTime::now_utc);
        sqlx::query("INSERT INTO operation_record(id,domain_type,operation_type,operation_name,operator_name,instance_id,business_project_id,state,target_count,started_at,heartbeat_at) VALUES(?,'smart_screen',?,?,?,?,?,'running',?,?,UTC_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE id=id")
            .bind(op.id).bind(op.action).bind(op.name).bind(op.operator).bind(op.instance_id).bind(op.business_project_id).bind(targets.len() as u32).bind(started).execute(&mut *tx).await.map_err(db)?;
        let row=sqlx::query("SELECT domain_type,operation_type,business_project_id,instance_id,target_count FROM operation_record WHERE id=? FOR UPDATE").bind(op.id).fetch_one(&mut *tx).await.map_err(db)?;
        if row.get::<String, _>("domain_type") != "smart_screen"
            || row.get::<String, _>("operation_type") != op.action
            || row
                .get::<Option<String>, _>("business_project_id")
                .as_deref()
                != Some(op.business_project_id)
            || row.get::<String, _>("instance_id") != op.instance_id
            || row.get::<u32, _>("target_count") != targets.len() as u32
        {
            return Err(AppError::Conflict(
                "同一共享请求不能替换项目、操作或来源电脑".into(),
            ));
        }
        let stored:Vec<String>=sqlx::query_scalar("SELECT resource_key FROM operation_target_result WHERE operation_id=? ORDER BY resource_key").bind(op.id).fetch_all(&mut *tx).await.map_err(db)?;
        if !stored.is_empty() && stored.iter().collect::<std::collections::BTreeSet<_>>() != targets
        {
            return Err(AppError::Conflict("同一共享请求不能替换目标".into()));
        }
        for id in &targets {
            sqlx::query("INSERT INTO operation_target_result(operation_id,resource_type,resource_key,result_state) VALUES(?,'smart_screen',?,'pending') ON DUPLICATE KEY UPDATE operation_id=operation_id")
                .bind(op.id).bind(id).execute(&mut *tx).await.map_err(db)?;
        }
        tx.commit().await.map_err(db)
    }

    pub async fn save_target(
        &self,
        operation: &str,
        business: &str,
        result: &ScreenTargetResult,
        state: &str,
    ) -> AppResult<()> {
        if !["succeeded", "failed", "cancelled", "interrupted"].contains(&state) {
            return Err(AppError::InvalidConfig(
                "共享结果必须是已确认的最终状态".into(),
            ));
        }
        // 完整实测和执行依据留本机；共享仅保留必要摘要。
        let brief: String = result.message.chars().take(1000).collect();
        let summary = serde_json::json!({"formatVersion":result.format_version,"screenId":result.screen_id,"device":result.device,"business":result.business,"shared":"succeeded","beforeAppVersion":result.before_app_version,"afterAppVersion":result.after_app_version,"observedAt":result.observation.as_ref().map(|o|&o.observed_at),"observedIp":result.observation.as_ref().map(|o|&o.observed_ip),"appVersionCode":result.observation.as_ref().and_then(|o|o.app_version_code),"message":brief});
        let mut summary = summary;
        if let Some(config) = result.evidence.get("config") {
            // 共享记录只保存字段名和处理结果，不复制地址与配置快照。
            summary["configuration"] = serde_json::json!({"fields":config["fields"],"save":config["save"],"savedFields":config["savedFields"],"failedFields":config["failedFields"],"restart":config["restart"],"readback":config["readback"]});
            summary["configuration"]["changes"] = serde_json::json!(crate::domain::smart_screen::app_config::shared_changes(config));
        }
        if let Some(package) = result.evidence.get("package") { summary["package"] = package.clone(); }
        if let Some(ntp) = result.evidence.get("ntp") {
            summary["ntp"] = serde_json::json!({"beforeServer":ntp["before"]["server"],"targetServer":ntp["targetServer"],"afterServer":ntp["after"]["server"],"autoTime":ntp["after"]["autoTime"],"save":ntp["save"],"activation":ntp["activation"],"sync":ntp["sync"],"rebootRequired":ntp["rebootRequired"],"clockOffsetSeconds":ntp["syncEvidence"]["clockOffsetSeconds"]});
        }
        summary["targetName"] = result
            .evidence
            .get("targetName")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        summary["targetIp"] = result
            .evidence
            .get("targetIp")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        validate_summary_json(&summary).map_err(map_formal_error)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        let row=sqlx::query("SELECT r.result_state,CAST(r.result_detail_json AS CHAR) AS result_detail_json FROM operation_target_result r JOIN operation_record o ON o.id=r.operation_id WHERE r.operation_id=? AND r.resource_type='smart_screen' AND r.resource_key=? AND o.domain_type='smart_screen' AND o.business_project_id=? FOR UPDATE")
            .bind(operation).bind(&result.screen_id).bind(business).fetch_optional(&mut *tx).await.map_err(db)?.ok_or_else(||AppError::NotFound("共享目标不属于当前项目和操作".into()))?;
        let previous: String = row.try_get("result_state").map_err(db)?;
        if previous != "pending" {
            let text: Option<String> = row.try_get("result_detail_json").map_err(db)?;
            let detail: Option<serde_json::Value> = text
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|_| AppError::Conflict("共享结果格式异常".into()))?;
            if previous == state && detail.as_ref() == Some(&summary) {
                return Ok(());
            }
            return Err(AppError::Conflict(
                "该操作结果已确认，不能被另一份结果覆盖".into(),
            ));
        }
        sqlx::query("UPDATE operation_target_result SET result_state=?,before_version=?,after_version=?,result_summary=?,result_detail_json=?,completed_at=UTC_TIMESTAMP(6) WHERE operation_id=? AND resource_type='smart_screen' AND resource_key=?")
            .bind(state).bind(&result.before_app_version).bind(&result.after_app_version).bind(&brief).bind(summary.to_string()).bind(operation).bind(&result.screen_id).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("UPDATE operation_record SET heartbeat_at=UTC_TIMESTAMP(6) WHERE id=?")
            .bind(operation)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)
    }

    pub async fn finish(&self, operation: &str, business: &str) -> AppResult<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let exists:Option<String>=sqlx::query_scalar("SELECT id FROM operation_record WHERE id=? AND domain_type='smart_screen' AND business_project_id=? FOR UPDATE").bind(operation).bind(business).fetch_optional(&mut *tx).await.map_err(db)?;
        if exists.is_none() {
            return Err(AppError::NotFound("共享操作不属于当前项目".into()));
        }
        let states: Vec<String> = sqlx::query_scalar(
            "SELECT result_state FROM operation_target_result WHERE operation_id=?",
        )
        .bind(operation)
        .fetch_all(&mut *tx)
        .await
        .map_err(db)?;
        if states.iter().any(|s| s == "pending") {
            return Err(AppError::Conflict("仍有目标未保存最终结果".into()));
        }
        let success = states.iter().filter(|s| s.as_str() == "succeeded").count() as u32;
        let cancel = states.iter().filter(|s| s.as_str() == "cancelled").count() as u32;
        let failed = states.len() as u32 - success - cancel;
        let state = if success == states.len() as u32 {
            "succeeded"
        } else if cancel == states.len() as u32 {
            "cancelled"
        } else if success > 0 {
            "partially_succeeded"
        } else {
            "failed"
        };
        sqlx::query("UPDATE operation_record SET state=?,success_count=?,failure_count=?,cancelled_count=?,ended_at=COALESCE(ended_at,UTC_TIMESTAMP(6)),heartbeat_at=UTC_TIMESTAMP(6) WHERE id=?")
            .bind(state).bind(success).bind(failed).bind(cancel).bind(operation).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
    }

    pub async fn audit(
        &self,
        id: &str,
        business: &str,
        object: &str,
        action: &str,
        operator: &str,
        instance: &str,
        fields: &serde_json::Value,
    ) -> AppResult<()> {
        validate_summary_json(fields).map_err(map_formal_error)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("INSERT INTO audit_event(id,domain_type,object_type,object_key,action,operator_name,instance_id,changed_fields_json,created_at,business_project_id,request_id) VALUES(?,'smart_screen','smart_screen',?,?,?,?,?,UTC_TIMESTAMP(6),?,?) ON DUPLICATE KEY UPDATE id=id")
            .bind(id).bind(object).bind(action).bind(operator).bind(instance).bind(fields.to_string()).bind(business).bind(id).execute(&mut *tx).await.map_err(db)?;
        let saved: (String, Option<String>) = sqlx::query_as(
            "SELECT object_key,business_project_id FROM audit_event WHERE id=? FOR UPDATE",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if saved.0 != object || saved.1.as_deref() != Some(business) {
            return Err(AppError::Conflict("修改记录与原请求不一致".into()));
        }
        tx.commit().await.map_err(db)
    }
}
