use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{MySqlPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use super::error::{FormalError, FormalResult};

const FORBIDDEN_SUMMARY_KEYS: &[&str] = &[
    "password",
    "privatekey",
    "authkey",
    "token",
    "localpath",
    "progress",
    "steps",
    "stdout",
    "stderr",
    "log",
];

#[derive(Clone, Debug)]
pub struct OperationStart {
    pub domain_type: String,
    pub operation_type: String,
    pub operation_name: String,
    pub operator_name: String,
    pub instance_id: String,
    pub targets: Vec<(String, String)>,
    pub artifact_name: Option<String>,
    pub artifact_version: Option<String>,
    pub operation_summary: Option<Value>,
    pub retry_of_operation_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OperationRecord {
    pub id: String,
    pub state: String,
    pub target_count: u32,
    pub success_count: u32,
    pub failure_count: u32,
    pub cancelled_count: u32,
    pub version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleOperationCandidate {
    pub id: String,
    pub version: u64,
    pub target_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetFinalResult {
    pub operation_id: String,
    pub resource_type: String,
    pub resource_key: String,
    pub result_state: String,
    pub before_version: Option<String>,
    pub after_version: Option<String>,
    pub result_summary: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationFinalResult {
    pub operation_id: String,
    pub expected_version: u64,
    pub state: String,
    pub result_summary: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OperationHistoryRecord {
    pub id: String,
    pub domain_type: String,
    pub operation_type: String,
    pub operation_name: String,
    pub operator_name: String,
    pub instance_id: String,
    pub state: String,
    pub target_count: u32,
    pub success_count: u32,
    pub failure_count: u32,
    pub cancelled_count: u32,
    pub artifact_name: Option<String>,
    pub artifact_version: Option<String>,
    pub started_at: OffsetDateTime,
    pub ended_at: Option<OffsetDateTime>,
    pub result_summary: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

#[derive(Clone, Debug)]
pub struct OperationHistoryTargetRecord {
    pub resource_type: String,
    pub resource_key: String,
    pub result_state: String,
    pub before_version: Option<String>,
    pub after_version: Option<String>,
    pub result_summary: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub completed_at: Option<OffsetDateTime>,
}

#[derive(Clone)]
pub struct OperationRepository {
    pool: MySqlPool,
}

impl OperationRepository {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    pub async fn list_history(
        &self,
        domain_type: &str,
        operation_type: Option<&str>,
        state: Option<&str>,
        page: u32,
        page_size: u32,
    ) -> FormalResult<(Vec<OperationHistoryRecord>, u64)> {
        self.list_history_scoped(domain_type,None,operation_type,state,page,page_size).await
    }

    pub async fn list_history_scoped(&self,domain_type:&str,business_project_id:Option<&str>,operation_type:Option<&str>,state:Option<&str>,page:u32,page_size:u32)->FormalResult<(Vec<OperationHistoryRecord>,u64)>{
        if domain_type.trim().is_empty()
            || page == 0
            || !(1..=100).contains(&page_size)
            || operation_type.is_some_and(str::is_empty)
            || state.is_some_and(str::is_empty)
        {
            return Err(FormalError::InvalidConfig("操作历史查询参数无效".into()));
        }
        let total = sqlx::query_scalar::<_, i64>(concat!(
            "SELECT COUNT(*) FROM operation_record WHERE domain_type = ? AND (? IS NULL OR business_project_id = ?) ",
            "AND (? IS NULL OR operation_type = ?) AND (? IS NULL OR state = ?)"
        ))
        .bind(domain_type)
        .bind(business_project_id)
        .bind(business_project_id)
        .bind(operation_type)
        .bind(operation_type)
        .bind(state)
        .bind(state)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| map_error("统计操作历史", error))?;
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let rows = sqlx::query(concat!(
            "SELECT id, domain_type, operation_type, operation_name, operator_name, instance_id, ",
            "state, target_count, success_count, failure_count, cancelled_count, artifact_name, ",
            "artifact_version, started_at, ended_at, result_summary, error_code, error_summary ",
            "FROM operation_record WHERE domain_type = ? AND (? IS NULL OR business_project_id = ?) ",
            "AND (? IS NULL OR operation_type = ?) AND (? IS NULL OR state = ?) ",
            "ORDER BY started_at DESC, id DESC LIMIT ? OFFSET ?"
        ))
        .bind(domain_type)
        .bind(business_project_id)
        .bind(business_project_id)
        .bind(operation_type)
        .bind(operation_type)
        .bind(state)
        .bind(state)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| map_error("查询操作历史", error))?;
        let items = rows
            .into_iter()
            .map(map_history_record)
            .collect::<FormalResult<Vec<_>>>()?;
        Ok((
            items,
            u64::try_from(total).map_err(|_| FormalError::LocalDatabase("解析操作历史总数"))?,
        ))
    }

    pub async fn history_detail(
        &self,
        domain_type: &str,
        operation_id: &str,
    ) -> FormalResult<(OperationHistoryRecord, Vec<OperationHistoryTargetRecord>)> {
        self.history_detail_scoped(domain_type,None,operation_id).await
    }

    pub async fn history_detail_scoped(&self,domain_type:&str,business_project_id:Option<&str>,operation_id:&str)->FormalResult<(OperationHistoryRecord,Vec<OperationHistoryTargetRecord>)>{
        if domain_type.trim().is_empty() || operation_id.trim().is_empty() {
            return Err(FormalError::InvalidConfig("操作历史详情参数无效".into()));
        }
        let row = sqlx::query(concat!(
            "SELECT id, domain_type, operation_type, operation_name, operator_name, instance_id, ",
            "state, target_count, success_count, failure_count, cancelled_count, artifact_name, ",
            "artifact_version, started_at, ended_at, result_summary, error_code, error_summary ",
            "FROM operation_record WHERE id = ? AND domain_type = ? AND (? IS NULL OR business_project_id = ?)"
        ))
        .bind(operation_id)
        .bind(domain_type)
        .bind(business_project_id)
        .bind(business_project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| map_error("读取操作历史详情", error))?
        .ok_or_else(|| FormalError::NotFound(format!("操作记录不存在：{operation_id}")))?;
        let operation = map_history_record(row)?;
        let targets = sqlx::query(concat!(
            "SELECT resource_type, resource_key, result_state, before_version, after_version, ",
            "result_summary, error_code, error_summary, completed_at ",
            "FROM operation_target_result WHERE operation_id = ? ",
            "ORDER BY resource_type, resource_key"
        ))
        .bind(operation_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| map_error("读取操作目标历史", error))?
        .into_iter()
        .map(map_history_target)
        .collect::<FormalResult<Vec<_>>>()?;
        Ok((operation, targets))
    }

    pub async fn start(&self, input: OperationStart) -> FormalResult<OperationRecord> {
        validate_start(&input)?;
        let id = Uuid::now_v7().to_string();
        let now = OffsetDateTime::now_utc();
        let summary_json = input
            .operation_summary
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| FormalError::InvalidConfig("操作摘要无法序列化".into()))?;
        let targets = input.targets.into_iter().collect::<BTreeSet<_>>();
        let mut transaction = self.pool.begin().await.map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "begin operation transaction failed");
            FormalError::LocalDatabase("开始操作记录事务")
        })?;
        sqlx::query(
            "INSERT INTO operation_record \
             (id, domain_type, operation_type, operation_name, operator_name, instance_id, \
              state, target_count, operation_summary_json, artifact_name, artifact_version, \
              started_at, heartbeat_at, retry_of_operation_id, version) \
             VALUES (?, ?, ?, ?, ?, ?, 'running', ?, ?, ?, ?, ?, ?, ?, 1)",
        )
        .bind(&id)
        .bind(&input.domain_type)
        .bind(&input.operation_type)
        .bind(&input.operation_name)
        .bind(&input.operator_name)
        .bind(&input.instance_id)
        .bind(targets.len() as u32)
        .bind(&summary_json)
        .bind(&input.artifact_name)
        .bind(&input.artifact_version)
        .bind(now)
        .bind(now)
        .bind(&input.retry_of_operation_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_error("创建操作记录", error))?;
        for (resource_type, resource_key) in targets {
            sqlx::query(
                "INSERT INTO operation_target_result \
                 (operation_id, resource_type, resource_key, result_state) \
                 VALUES (?, ?, ?, 'pending')",
            )
            .bind(&id)
            .bind(resource_type)
            .bind(resource_key)
            .execute(&mut *transaction)
            .await
            .map_err(|error| map_error("创建操作目标结果", error))?;
        }
        transaction.commit().await.map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "commit operation transaction failed");
            FormalError::LocalDatabase("提交操作记录事务")
        })?;
        self.get(&id).await
    }

    pub async fn heartbeat(&self, operation_id: &str, expected_version: u64) -> FormalResult<u64> {
        let result = sqlx::query(
            "UPDATE operation_record SET heartbeat_at = UTC_TIMESTAMP(6), version = version + 1 \
             WHERE id = ? AND state = 'running' AND version = ?",
        )
        .bind(operation_id)
        .bind(expected_version)
        .execute(&self.pool)
        .await
        .map_err(|error| map_error("更新操作心跳", error))?;
        if result.rows_affected() != 1 {
            return Err(FormalError::Conflict("操作记录版本或状态已变化".into()));
        }
        Ok(expected_version + 1)
    }

    pub async fn finalize_target(&self, result: TargetFinalResult) -> FormalResult<()> {
        validate_target_result(&result)?;
        let updated = sqlx::query(
            "UPDATE operation_target_result SET result_state = ?, before_version = ?, \
             after_version = ?, result_summary = ?, error_code = ?, error_summary = ?, \
             completed_at = UTC_TIMESTAMP(6) WHERE operation_id = ? AND resource_type = ? \
             AND resource_key = ? AND result_state = 'pending'",
        )
        .bind(&result.result_state)
        .bind(&result.before_version)
        .bind(&result.after_version)
        .bind(&result.result_summary)
        .bind(&result.error_code)
        .bind(&result.error_summary)
        .bind(&result.operation_id)
        .bind(&result.resource_type)
        .bind(&result.resource_key)
        .execute(&self.pool)
        .await
        .map_err(|error| map_error("写入节点最终结果", error))?;
        if updated.rows_affected() != 1 {
            return Err(FormalError::Conflict("节点结果已最终化或目标不存在".into()));
        }
        Ok(())
    }

    pub async fn finalize(&self, result: OperationFinalResult) -> FormalResult<OperationRecord> {
        validate_operation_state(&result.state)?;
        let mut transaction = self.pool.begin().await.map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "begin operation finalization failed");
            FormalError::LocalDatabase("开始操作最终化事务")
        })?;
        let rows = sqlx::query(
            "SELECT result_state, COUNT(*) AS count FROM operation_target_result \
             WHERE operation_id = ? GROUP BY result_state",
        )
        .bind(&result.operation_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| map_error("统计节点最终结果", error))?;
        let mut success = 0_u32;
        let mut failure = 0_u32;
        let mut cancelled = 0_u32;
        let mut pending = 0_u32;
        for row in rows {
            let state: String = row
                .try_get("result_state")
                .map_err(|_| FormalError::LocalDatabase("解析节点结果状态"))?;
            let count: i64 = row
                .try_get("count")
                .map_err(|_| FormalError::LocalDatabase("解析节点结果数量"))?;
            let count = u32::try_from(count)
                .map_err(|_| FormalError::LocalDatabase("节点结果数量超出范围"))?;
            match state.as_str() {
                "succeeded" => success += count,
                "failed" | "interrupted" | "unknown" => failure += count,
                "cancelled" => cancelled += count,
                "pending" => pending += count,
                _ => return Err(FormalError::InvalidConfig("存在未知节点结果状态".into())),
            }
        }
        if pending != 0 {
            return Err(FormalError::Conflict("仍有节点未形成最终结果".into()));
        }
        let updated = sqlx::query(
            "UPDATE operation_record SET state = ?, success_count = ?, failure_count = ?, \
             cancelled_count = ?, result_summary = ?, error_code = ?, error_summary = ?, \
             ended_at = UTC_TIMESTAMP(6), heartbeat_at = UTC_TIMESTAMP(6), version = version + 1 \
             WHERE id = ? AND state = 'running' AND version = ?",
        )
        .bind(&result.state)
        .bind(success)
        .bind(failure)
        .bind(cancelled)
        .bind(&result.result_summary)
        .bind(&result.error_code)
        .bind(&result.error_summary)
        .bind(&result.operation_id)
        .bind(result.expected_version)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_error("最终化操作记录", error))?;
        if updated.rows_affected() != 1 {
            return Err(FormalError::Conflict("操作记录已被其他实例更新".into()));
        }
        transaction.commit().await.map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "commit operation finalization failed");
            FormalError::LocalDatabase("提交操作最终化事务")
        })?;
        self.get(&result.operation_id).await
    }

    pub async fn get(&self, operation_id: &str) -> FormalResult<OperationRecord> {
        let row = sqlx::query(
            "SELECT id, state, target_count, success_count, failure_count, cancelled_count, version \
             FROM operation_record WHERE id = ?",
        )
        .bind(operation_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| map_error("读取操作记录", error))?
        .ok_or_else(|| FormalError::NotFound(format!("操作记录不存在：{operation_id}")))?;
        Ok(OperationRecord {
            id: row
                .try_get("id")
                .map_err(|_| FormalError::LocalDatabase("解析操作ID"))?,
            state: row
                .try_get("state")
                .map_err(|_| FormalError::LocalDatabase("解析操作状态"))?,
            target_count: row
                .try_get("target_count")
                .map_err(|_| FormalError::LocalDatabase("解析目标数量"))?,
            success_count: row
                .try_get("success_count")
                .map_err(|_| FormalError::LocalDatabase("解析成功数量"))?,
            failure_count: row
                .try_get("failure_count")
                .map_err(|_| FormalError::LocalDatabase("解析失败数量"))?,
            cancelled_count: row
                .try_get("cancelled_count")
                .map_err(|_| FormalError::LocalDatabase("解析取消数量"))?,
            version: row
                .try_get("version")
                .map_err(|_| FormalError::LocalDatabase("解析操作版本"))?,
        })
    }

    pub async fn list_stale_candidates(
        &self,
        stale_after: std::time::Duration,
        limit: u32,
    ) -> FormalResult<Vec<StaleOperationCandidate>> {
        let rows = sqlx::query(
            "SELECT operation.id, operation.version, operation.target_count \
             FROM operation_record operation WHERE operation.state = 'running' \
             AND operation.heartbeat_at < DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? SECOND) \
             AND NOT EXISTS (SELECT 1 FROM resource_lease lease_row \
                 WHERE lease_row.operation_id = operation.id \
                 AND lease_row.lease_state = 'active' \
                 AND lease_row.expires_at > UTC_TIMESTAMP(6)) \
             ORDER BY operation.heartbeat_at ASC, operation.id ASC LIMIT ?",
        )
        .bind(stale_after.as_secs().max(1) as i64)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await
        .map_err(|error| map_error("查询中断操作候选", error))?;
        rows.into_iter()
            .map(|row| {
                Ok(StaleOperationCandidate {
                    id: row
                        .try_get("id")
                        .map_err(|_| FormalError::LocalDatabase("解析中断候选ID"))?,
                    version: row
                        .try_get("version")
                        .map_err(|_| FormalError::LocalDatabase("解析中断候选版本"))?,
                    target_count: row
                        .try_get("target_count")
                        .map_err(|_| FormalError::LocalDatabase("解析中断候选目标数"))?,
                })
            })
            .collect()
    }

    pub async fn interrupt_stale(
        &self,
        operation_id: &str,
        expected_version: u64,
        stale_after: std::time::Duration,
    ) -> FormalResult<OperationRecord> {
        let stale_seconds = stale_after.as_secs().max(1) as i64;
        let mut transaction = self.pool.begin().await.map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "begin stale operation recovery failed");
            FormalError::LocalDatabase("开始中断操作恢复事务")
        })?;
        let eligible = sqlx::query_scalar::<_, i8>(
            "SELECT EXISTS(SELECT 1 FROM operation_record operation \
             WHERE operation.id = ? AND operation.state = 'running' AND operation.version = ? \
             AND operation.heartbeat_at < DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? SECOND) \
             AND NOT EXISTS (SELECT 1 FROM resource_lease lease_row \
                 WHERE lease_row.operation_id = operation.id \
                 AND lease_row.lease_state = 'active' \
                 AND lease_row.expires_at > UTC_TIMESTAMP(6)))",
        )
        .bind(operation_id)
        .bind(expected_version)
        .bind(stale_seconds)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| map_error("校验中断操作候选", error))?;
        if eligible == 0 {
            return Err(FormalError::Conflict(
                "操作仍有有效心跳、活动租约或版本已变化".into(),
            ));
        }
        sqlx::query(
            "UPDATE operation_target_result SET result_state = 'unknown', \
             result_summary = '发起实例中断，需人工对账后重试', \
             error_code = 'APP_INSTANCE_LOST', error_summary = '未形成可确认的节点最终结果', \
             completed_at = UTC_TIMESTAMP(6) WHERE operation_id = ? AND result_state = 'pending'",
        )
        .bind(operation_id)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_error("标记未知节点结果", error))?;
        let rows = sqlx::query(
            "SELECT result_state, COUNT(*) AS count FROM operation_target_result \
             WHERE operation_id = ? GROUP BY result_state",
        )
        .bind(operation_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| map_error("统计中断操作结果", error))?;
        let mut success = 0_u32;
        let mut failure = 0_u32;
        let mut cancelled = 0_u32;
        for row in rows {
            let state: String = row
                .try_get("result_state")
                .map_err(|_| FormalError::LocalDatabase("解析中断节点状态"))?;
            let count: i64 = row
                .try_get("count")
                .map_err(|_| FormalError::LocalDatabase("解析中断节点数量"))?;
            let count = u32::try_from(count)
                .map_err(|_| FormalError::LocalDatabase("中断节点数量超出范围"))?;
            match state.as_str() {
                "succeeded" => success += count,
                "failed" | "interrupted" | "unknown" => failure += count,
                "cancelled" => cancelled += count,
                _ => return Err(FormalError::Conflict("仍有节点未完成中断对账".into())),
            }
        }
        let updated = sqlx::query(
            "UPDATE operation_record SET state = 'interrupted', success_count = ?, \
             failure_count = ?, cancelled_count = ?, result_summary = '发起实例失联，操作已中断', \
             error_code = 'APP_INSTANCE_LOST', error_summary = '请核对远端实际状态后显式重试', \
             ended_at = UTC_TIMESTAMP(6), heartbeat_at = UTC_TIMESTAMP(6), version = version + 1 \
             WHERE id = ? AND state = 'running' AND version = ? \
             AND heartbeat_at < DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? SECOND) \
             AND NOT EXISTS (SELECT 1 FROM resource_lease lease_row \
                 WHERE lease_row.operation_id = operation_record.id \
                 AND lease_row.lease_state = 'active' \
                 AND lease_row.expires_at > UTC_TIMESTAMP(6))",
        )
        .bind(success)
        .bind(failure)
        .bind(cancelled)
        .bind(operation_id)
        .bind(expected_version)
        .bind(stale_seconds)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_error("中断失联操作", error))?;
        if updated.rows_affected() != 1 {
            return Err(FormalError::Conflict("操作恢复期间状态已变化".into()));
        }
        transaction.commit().await.map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "commit stale operation recovery failed");
            FormalError::LocalDatabase("提交中断操作恢复事务")
        })?;
        self.get(operation_id).await
    }

    pub async fn delete_test_operation(&self, operation_id: &str) -> FormalResult<()> {
        sqlx::query("DELETE FROM resource_lease WHERE operation_id = ?")
            .bind(operation_id)
            .execute(&self.pool)
            .await
            .map_err(|error| map_error("清理操作租约", error))?;
        sqlx::query("DELETE FROM operation_record WHERE id = ?")
            .bind(operation_id)
            .execute(&self.pool)
            .await
            .map_err(|error| map_error("清理操作记录", error))?;
        Ok(())
    }
}

pub fn validate_summary_json(value: &Value) -> FormalResult<()> {
    fn canonical_key(value: &str) -> String {
        value
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    }

    fn visit(value: &Value) -> bool {
        match value {
            Value::Object(map) => map.iter().any(|(key, value)| {
                let normalized = canonical_key(key);
                FORBIDDEN_SUMMARY_KEYS
                    .iter()
                    .any(|forbidden| normalized.contains(forbidden))
                    || visit(value)
            }),
            Value::Array(values) => values.iter().any(visit),
            _ => false,
        }
    }
    if visit(value) {
        return Err(FormalError::InvalidConfig(
            "操作摘要包含过程数据或敏感字段".into(),
        ));
    }
    Ok(())
}

fn map_history_record(row: sqlx::mysql::MySqlRow) -> FormalResult<OperationHistoryRecord> {
    Ok(OperationHistoryRecord {
        id: row
            .try_get("id")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史ID"))?,
        domain_type: row
            .try_get("domain_type")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史领域"))?,
        operation_type: row
            .try_get("operation_type")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史类型"))?,
        operation_name: row
            .try_get("operation_name")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史名称"))?,
        operator_name: row
            .try_get("operator_name")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史操作人"))?,
        instance_id: row
            .try_get("instance_id")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史实例"))?,
        state: row
            .try_get("state")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史状态"))?,
        target_count: row
            .try_get("target_count")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史目标数"))?,
        success_count: row
            .try_get("success_count")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史成功数"))?,
        failure_count: row
            .try_get("failure_count")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史失败数"))?,
        cancelled_count: row
            .try_get("cancelled_count")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史取消数"))?,
        artifact_name: row
            .try_get("artifact_name")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史制品名称"))?,
        artifact_version: row
            .try_get("artifact_version")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史制品版本"))?,
        started_at: row
            .try_get("started_at")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史开始时间"))?,
        ended_at: row
            .try_get("ended_at")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史结束时间"))?,
        result_summary: row
            .try_get("result_summary")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史结果摘要"))?,
        error_code: row
            .try_get("error_code")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史错误码"))?,
        error_summary: row
            .try_get("error_summary")
            .map_err(|_| FormalError::LocalDatabase("解析操作历史错误摘要"))?,
    })
}

fn map_history_target(row: sqlx::mysql::MySqlRow) -> FormalResult<OperationHistoryTargetRecord> {
    Ok(OperationHistoryTargetRecord {
        resource_type: row
            .try_get("resource_type")
            .map_err(|_| FormalError::LocalDatabase("解析操作目标类型"))?,
        resource_key: row
            .try_get("resource_key")
            .map_err(|_| FormalError::LocalDatabase("解析操作目标标识"))?,
        result_state: row
            .try_get("result_state")
            .map_err(|_| FormalError::LocalDatabase("解析操作目标状态"))?,
        before_version: row
            .try_get("before_version")
            .map_err(|_| FormalError::LocalDatabase("解析操作目标原版本"))?,
        after_version: row
            .try_get("after_version")
            .map_err(|_| FormalError::LocalDatabase("解析操作目标新版本"))?,
        result_summary: row
            .try_get("result_summary")
            .map_err(|_| FormalError::LocalDatabase("解析操作目标摘要"))?,
        error_code: row
            .try_get("error_code")
            .map_err(|_| FormalError::LocalDatabase("解析操作目标错误码"))?,
        error_summary: row
            .try_get("error_summary")
            .map_err(|_| FormalError::LocalDatabase("解析操作目标错误摘要"))?,
        completed_at: row
            .try_get("completed_at")
            .map_err(|_| FormalError::LocalDatabase("解析操作目标完成时间"))?,
    })
}

fn validate_start(input: &OperationStart) -> FormalResult<()> {
    if input.domain_type.is_empty()
        || input.operation_type.is_empty()
        || input.operation_name.is_empty()
        || input.operator_name.is_empty()
        || input.instance_id.is_empty()
        || input.targets.is_empty()
    {
        return Err(FormalError::InvalidConfig("操作记录参数不完整".into()));
    }
    if let Some(summary) = &input.operation_summary {
        validate_summary_json(summary)?;
    }
    Ok(())
}

fn validate_target_result(result: &TargetFinalResult) -> FormalResult<()> {
    if result.operation_id.is_empty()
        || result.resource_type.is_empty()
        || result.resource_key.is_empty()
        || !matches!(
            result.result_state.as_str(),
            "succeeded" | "failed" | "cancelled" | "interrupted" | "unknown"
        )
    {
        return Err(FormalError::InvalidConfig("节点最终结果参数无效".into()));
    }
    Ok(())
}

fn validate_operation_state(state: &str) -> FormalResult<()> {
    if matches!(
        state,
        "succeeded" | "partially_succeeded" | "failed" | "cancelled" | "interrupted"
    ) {
        Ok(())
    } else {
        Err(FormalError::InvalidConfig("操作最终状态无效".into()))
    }
}

fn map_error(operation: &'static str, error: sqlx::Error) -> FormalError {
    tracing::error!(operation, error = ?crate::core::log_safety::safe_error(&error), "operation mysql call failed");
    FormalError::LocalDatabase(operation)
}
