use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use time::OffsetDateTime;

use crate::core::error::{AppError, AppResult};
use crate::domain::common::task::{
    StepState, TargetState, TaskRecord, TaskState, TaskStepRecord, TaskTargetRecord,
};

#[derive(Clone, Debug)]
pub struct CreateTask {
    pub id: String,
    pub local_project_id: String,
    pub remote_operation_record_id: Option<String>,
    pub domain_type: String,
    pub operation_type: String,
    pub name: String,
    pub priority: i32,
    pub batch_size: u32,
    pub concurrency: u32,
    pub payload_ref: Option<String>,
    pub log_path: String,
    pub targets: Vec<(String, String)>,
}

impl CreateTask {
    fn validate(&self) -> AppResult<()> {
        if self.id.trim().is_empty()
            || self.local_project_id.trim().is_empty()
            || self.domain_type.trim().is_empty()
            || self.operation_type.trim().is_empty()
            || self.name.trim().is_empty()
            || self.log_path.trim().is_empty()
            || self.targets.is_empty()
            || self.batch_size == 0
            || self.concurrency == 0
            || self.concurrency > self.batch_size
        {
            return Err(AppError::InvalidConfig("本地任务参数不完整".into()));
        }
        if self.targets.iter().any(|(resource_type, resource_key)| {
            resource_type.trim().is_empty() || resource_key.trim().is_empty()
        }) {
            return Err(AppError::InvalidConfig("本地任务目标无效".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetUpdate {
    pub resource_type: String,
    pub resource_key: String,
    pub state: TargetState,
    pub stage: String,
    pub progress_current: u64,
    pub progress_total: u64,
    pub fencing_token: Option<u64>,
    pub message_code: Option<String>,
    pub message_params_json: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskStepWrite {
    pub id: String,
    pub resource_type: Option<String>,
    pub resource_key: Option<String>,
    pub step_code: String,
    pub state: StepState,
    pub error_code: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct TaskRepository {
    pool: SqlitePool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActiveTaskCounts {
    pub total: u32,
    pub queued: u32,
    pub running: u32,
}

impl TaskRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn active_counts(&self) -> AppResult<ActiveTaskCounts> {
        let rows = sqlx::query(concat!(
            "SELECT state, COUNT(*) AS count FROM local_task ",
            "WHERE state IN ('queued', 'running', 'cancelling', 'finalizing_failed') ",
            "GROUP BY state"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("统计活动任务", &error))?;
        let mut counts = ActiveTaskCounts::default();
        for row in rows {
            let state: String = row
                .try_get("state")
                .map_err(|error| AppError::database("解析活动任务状态", &error))?;
            let count: i64 = row
                .try_get("count")
                .map_err(|error| AppError::database("解析活动任务数量", &error))?;
            let count = u32::try_from(count)
                .map_err(|_| AppError::InvalidConfig("活动任务数量超出范围".into()))?;
            counts.total = counts.total.saturating_add(count);
            match state.as_str() {
                "queued" => counts.queued = counts.queued.saturating_add(count),
                "running" | "cancelling" | "finalizing_failed" => {
                    counts.running = counts.running.saturating_add(count);
                }
                _ => {}
            }
        }
        Ok(counts)
    }

    pub async fn has_active_for_project(&self, local_project_id: &str) -> AppResult<bool> {
        if local_project_id.trim().is_empty() {
            return Err(AppError::InvalidConfig("项目ID不能为空".into()));
        }
        let active = sqlx::query_scalar::<_, i64>(concat!(
            "SELECT COUNT(*) FROM local_task WHERE local_project_id = ? AND state IN ",
            "('draft', 'checking', 'ready', 'queued', 'running', 'cancelling', ",
            "'finalizing_failed')"
        ))
        .bind(local_project_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| AppError::database("检查项目活动任务", &error))?;
        Ok(active > 0)
    }

    pub async fn list_active(&self) -> AppResult<Vec<TaskRecord>> {
        let rows = sqlx::query(concat!(
            "SELECT id, local_project_id, remote_operation_record_id, domain_type, operation_type, ",
            "name, state, priority, batch_size, concurrency, payload_ref, sequence, log_path, ",
            "error_code, message, created_at, started_at, ended_at, updated_at ",
            "FROM local_task WHERE state IN ",
            "('queued', 'running', 'cancelling', 'finalizing_failed') ",
            "ORDER BY updated_at, id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("查询全部活动任务", &error))?;
        rows.into_iter().map(map_task).collect()
    }

    pub async fn create(&self, input: CreateTask) -> AppResult<TaskRecord> {
        input.validate()?;
        let targets = input.targets.into_iter().collect::<BTreeSet<_>>();
        let now = timestamp();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| AppError::database("开始创建本地任务事务", &error))?;
        sqlx::query(
            "INSERT INTO local_task \
             (id, local_project_id, remote_operation_record_id, domain_type, operation_type, \
              name, state, priority, batch_size, concurrency, payload_ref, sequence, log_path, \
              created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, 'draft', ?, ?, ?, ?, 0, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.local_project_id)
        .bind(&input.remote_operation_record_id)
        .bind(&input.domain_type)
        .bind(&input.operation_type)
        .bind(&input.name)
        .bind(input.priority)
        .bind(i64::from(input.batch_size))
        .bind(i64::from(input.concurrency))
        .bind(&input.payload_ref)
        .bind(&input.log_path)
        .bind(&now)
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| AppError::database("创建本地任务", &error))?;
        for (resource_type, resource_key) in targets {
            sqlx::query(
                "INSERT INTO local_task_target \
                 (local_task_id, resource_type, resource_key, state, stage, progress, \
                  progress_current, progress_total, updated_at) \
                 VALUES (?, ?, ?, 'pending', '', 0, 0, 0, ?)",
            )
            .bind(&input.id)
            .bind(resource_type)
            .bind(resource_key)
            .bind(&now)
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("创建本地任务目标", &error))?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交创建本地任务事务", &error))?;
        self.get(&input.id).await
    }

    pub async fn get(&self, task_id: &str) -> AppResult<TaskRecord> {
        let row = sqlx::query(
            "SELECT id, local_project_id, remote_operation_record_id, domain_type, operation_type, \
             name, state, priority, batch_size, concurrency, payload_ref, sequence, log_path, \
             error_code, message, created_at, started_at, ended_at, updated_at \
             FROM local_task WHERE id = ?",
        )
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::database("读取本地任务", &error))?
        .ok_or_else(|| AppError::NotFound(format!("本地任务不存在：{task_id}")))?;
        map_task(row)
    }

    pub async fn link_operation(&self, task_id: &str, operation_id: &str) -> AppResult<TaskRecord> {
        if task_id.trim().is_empty() || operation_id.trim().is_empty() {
            return Err(AppError::InvalidConfig("任务与操作关联参数无效".into()));
        }
        let now = timestamp();
        let result = sqlx::query(concat!(
            "UPDATE local_task SET remote_operation_record_id = ?, sequence = sequence + 1, ",
            "updated_at = ? WHERE id = ? AND remote_operation_record_id IS NULL"
        ))
        .bind(operation_id)
        .bind(&now)
        .bind(task_id)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::database("关联本地任务与共享操作", &error))?;
        if result.rows_affected() != 1 {
            return Err(AppError::Conflict(format!(
                "本地任务已关联其他操作或不存在：{task_id}"
            )));
        }
        self.get(task_id).await
    }

    pub async fn list_recent(
        &self,
        local_project_id: &str,
        limit: u32,
    ) -> AppResult<Vec<TaskRecord>> {
        if local_project_id.trim().is_empty() || limit == 0 || limit > 500 {
            return Err(AppError::InvalidConfig("任务列表查询参数无效".into()));
        }
        let rows = sqlx::query(
            "SELECT id, local_project_id, remote_operation_record_id, domain_type, operation_type, \
             name, state, priority, batch_size, concurrency, payload_ref, sequence, log_path, \
             error_code, message, created_at, started_at, ended_at, updated_at \
             FROM local_task WHERE local_project_id = ? \
             ORDER BY CASE state WHEN 'running' THEN 0 WHEN 'cancelling' THEN 1 \
              WHEN 'queued' THEN 2 WHEN 'finalizing_failed' THEN 3 ELSE 4 END, \
              updated_at DESC, id DESC LIMIT ?",
        )
        .bind(local_project_id)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("查询本地任务列表", &error))?;
        rows.into_iter().map(map_task).collect()
    }

    pub async fn list_artifact_cleanup_candidates(&self) -> AppResult<Vec<TaskRecord>> {
        let rows = sqlx::query(
            "SELECT id, local_project_id, remote_operation_record_id, domain_type, operation_type, \
             name, state, priority, batch_size, concurrency, payload_ref, sequence, log_path, \
             error_code, message, created_at, started_at, ended_at, updated_at \
             FROM local_task WHERE state IN \
             ('check_failed', 'cancelled', 'succeeded', 'partially_succeeded', 'failed', \
              'interrupted') ORDER BY updated_at, id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("查询任务制品清理候选", &error))?;
        rows.into_iter().map(map_task).collect()
    }

    pub async fn targets(&self, task_id: &str) -> AppResult<Vec<TaskTargetRecord>> {
        let rows = sqlx::query(
            "SELECT local_task_id, resource_type, resource_key, state, stage, progress_current, \
             progress_total, fencing_token, message_code, message_params_json, updated_at \
             FROM local_task_target WHERE local_task_id = ? ORDER BY resource_type, resource_key",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("查询本地任务目标", &error))?;
        rows.into_iter().map(map_target).collect()
    }

    pub async fn steps(&self, task_id: &str) -> AppResult<Vec<TaskStepRecord>> {
        let rows = sqlx::query(
            "SELECT id, local_task_id, resource_type, resource_key, step_code, state, error_code, \
             message, started_at, ended_at, updated_at FROM local_task_step \
             WHERE local_task_id = ? ORDER BY started_at, id",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("查询本地任务步骤", &error))?;
        rows.into_iter().map(map_step).collect()
    }

    pub async fn transition(
        &self,
        task_id: &str,
        expected: TaskState,
        next: TaskState,
        error_code: Option<&str>,
        message: Option<&str>,
    ) -> AppResult<TaskRecord> {
        expected.ensure_transition(next)?;
        let now = timestamp();
        let started_at = (next == TaskState::Running).then_some(now.as_str());
        let ended_at = next.is_terminal().then_some(now.as_str());
        let result = sqlx::query(
            "UPDATE local_task SET state = ?, error_code = ?, message = ?, \
             started_at = COALESCE(started_at, ?), ended_at = COALESCE(?, ended_at), \
             sequence = sequence + 1, updated_at = ? WHERE id = ? AND state = ?",
        )
        .bind(next.as_str())
        .bind(error_code)
        .bind(message)
        .bind(started_at)
        .bind(ended_at)
        .bind(&now)
        .bind(task_id)
        .bind(expected.as_str())
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::database("更新本地任务状态", &error))?;
        if result.rows_affected() != 1 {
            return Err(AppError::Conflict(format!("任务状态已变化：{task_id}")));
        }
        self.get(task_id).await
    }

    pub async fn update_target(&self, task_id: &str, update: TargetUpdate) -> AppResult<u64> {
        if update.resource_type.trim().is_empty()
            || update.resource_key.trim().is_empty()
            || (update.progress_total > 0 && update.progress_current > update.progress_total)
        {
            return Err(AppError::InvalidConfig("任务目标更新参数无效".into()));
        }
        let percent = if update.progress_total == 0 {
            0
        } else {
            update.progress_current.saturating_mul(100) / update.progress_total
        }
        .min(100);
        let now = timestamp();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| AppError::database("开始更新任务目标事务", &error))?;
        let result = sqlx::query(
            "UPDATE local_task_target SET state = ?, stage = ?, progress = ?, \
             progress_current = ?, progress_total = ?, fencing_token = COALESCE(?, fencing_token), message_code = ?, \
             message_params_json = ?, updated_at = ? \
             WHERE local_task_id = ? AND resource_type = ? AND resource_key = ?",
        )
        .bind(update.state.as_str())
        .bind(&update.stage)
        .bind(to_i64(percent, "任务目标百分比")?)
        .bind(to_i64(update.progress_current, "任务目标当前进度")?)
        .bind(to_i64(update.progress_total, "任务目标总进度")?)
        .bind(
            update
                .fencing_token
                .map(|value| to_i64(value, "fencing token"))
                .transpose()?,
        )
        .bind(&update.message_code)
        .bind(&update.message_params_json)
        .bind(&now)
        .bind(task_id)
        .bind(&update.resource_type)
        .bind(&update.resource_key)
        .execute(&mut *transaction)
        .await
        .map_err(|error| AppError::database("更新任务目标", &error))?;
        if result.rows_affected() != 1 {
            return Err(AppError::NotFound(format!(
                "任务目标不存在：{}/{}",
                update.resource_type, update.resource_key
            )));
        }
        let sequence = next_sequence(&mut transaction, task_id, &now).await?;
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交更新任务目标事务", &error))?;
        Ok(sequence)
    }

    pub async fn save_step(&self, task_id: &str, step: TaskStepWrite) -> AppResult<u64> {
        if step.id.trim().is_empty() || step.step_code.trim().is_empty() {
            return Err(AppError::InvalidConfig("任务步骤参数无效".into()));
        }
        let now = timestamp();
        let started_at = (step.state == StepState::Running).then_some(now.as_str());
        let ended_at = matches!(
            step.state,
            StepState::Succeeded
                | StepState::Failed
                | StepState::Skipped
                | StepState::Cancelled
                | StepState::Interrupted
        )
        .then_some(now.as_str());
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| AppError::database("开始保存任务步骤事务", &error))?;
        sqlx::query(
            "INSERT INTO local_task_step \
             (id, local_task_id, resource_type, resource_key, step_code, state, error_code, \
              message, started_at, ended_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET state = excluded.state, error_code = excluded.error_code, \
              message = excluded.message, started_at = COALESCE(local_task_step.started_at, excluded.started_at), \
              ended_at = excluded.ended_at, updated_at = excluded.updated_at",
        )
        .bind(&step.id)
        .bind(task_id)
        .bind(&step.resource_type)
        .bind(&step.resource_key)
        .bind(&step.step_code)
        .bind(step.state.as_str())
        .bind(&step.error_code)
        .bind(&step.message)
        .bind(started_at)
        .bind(ended_at)
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| AppError::database("保存任务步骤", &error))?;
        let sequence = next_sequence(&mut transaction, task_id, &now).await?;
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交保存任务步骤事务", &error))?;
        Ok(sequence)
    }

    pub async fn finalize_projection(
        &self,
        task_id: &str,
        expected_state: TaskState,
        final_state: TaskState,
        targets: &[TargetUpdate],
        steps: &[TaskStepWrite],
    ) -> AppResult<()> {
        expected_state.ensure_transition(final_state)?;
        if !final_state.is_terminal() || targets.is_empty() {
            return Err(AppError::InvalidConfig("本地最终化投影参数无效".into()));
        }
        let now = timestamp();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| AppError::database("开始本地最终化投影事务", &error))?;
        for update in targets {
            if update.resource_type.trim().is_empty()
                || update.resource_key.trim().is_empty()
                || (update.progress_total > 0 && update.progress_current > update.progress_total)
            {
                return Err(AppError::InvalidConfig("本地最终化目标参数无效".into()));
            }
            let percent = if update.progress_total == 0 {
                0
            } else {
                update.progress_current.saturating_mul(100) / update.progress_total
            }
            .min(100);
            let result = sqlx::query(concat!(
                "UPDATE local_task_target SET state = ?, stage = ?, progress = ?, ",
                "progress_current = ?, progress_total = ?, ",
                "fencing_token = COALESCE(?, fencing_token), message_code = ?, ",
                "message_params_json = ?, updated_at = ? ",
                "WHERE local_task_id = ? AND resource_type = ? AND resource_key = ?"
            ))
            .bind(update.state.as_str())
            .bind(&update.stage)
            .bind(to_i64(percent, "任务目标百分比")?)
            .bind(to_i64(update.progress_current, "任务目标当前进度")?)
            .bind(to_i64(update.progress_total, "任务目标总进度")?)
            .bind(
                update
                    .fencing_token
                    .map(|value| to_i64(value, "fencing token"))
                    .transpose()?,
            )
            .bind(&update.message_code)
            .bind(&update.message_params_json)
            .bind(&now)
            .bind(task_id)
            .bind(&update.resource_type)
            .bind(&update.resource_key)
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("写入本地最终化目标", &error))?;
            if result.rows_affected() != 1 {
                return Err(AppError::NotFound(format!(
                    "本地最终化目标不存在：{}/{}",
                    update.resource_type, update.resource_key
                )));
            }
        }
        for step in steps {
            if step.id.trim().is_empty() || step.step_code.trim().is_empty() {
                return Err(AppError::InvalidConfig("本地最终化步骤参数无效".into()));
            }
            let ended_at = matches!(
                step.state,
                StepState::Succeeded
                    | StepState::Failed
                    | StepState::Skipped
                    | StepState::Cancelled
                    | StepState::Interrupted
            )
            .then_some(now.as_str());
            sqlx::query(concat!(
                "INSERT INTO local_task_step ",
                "(id, local_task_id, resource_type, resource_key, step_code, state, error_code, ",
                "message, started_at, ended_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ",
                "ON CONFLICT(id) DO UPDATE SET state = excluded.state, ",
                "error_code = excluded.error_code, message = excluded.message, ",
                "started_at = COALESCE(local_task_step.started_at, excluded.started_at), ",
                "ended_at = excluded.ended_at, updated_at = excluded.updated_at"
            ))
            .bind(&step.id)
            .bind(task_id)
            .bind(&step.resource_type)
            .bind(&step.resource_key)
            .bind(&step.step_code)
            .bind(step.state.as_str())
            .bind(&step.error_code)
            .bind(&step.message)
            .bind(now.as_str())
            .bind(ended_at)
            .bind(&now)
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("写入本地最终化步骤", &error))?;
        }
        let updated = sqlx::query(concat!(
            "UPDATE local_task SET state = ?, error_code = NULL, message = NULL, ",
            "ended_at = COALESCE(ended_at, ?), sequence = sequence + 1, updated_at = ? ",
            "WHERE id = ? AND state = ?"
        ))
        .bind(final_state.as_str())
        .bind(&now)
        .bind(&now)
        .bind(task_id)
        .bind(expected_state.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(|error| AppError::database("写入本地任务最终状态", &error))?;
        if updated.rows_affected() != 1 {
            return Err(AppError::Conflict(format!(
                "本地任务最终化期间状态已变化：{task_id}"
            )));
        }
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交本地最终化投影事务", &error))
    }

    pub async fn recover_interrupted(&self) -> AppResult<Vec<String>> {
        let ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM local_task WHERE state IN \
             ('queued', 'running', 'cancelling') ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("查询待恢复本地任务", &error))?;
        if ids.is_empty() {
            return Ok(ids);
        }
        let now = timestamp();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| AppError::database("开始恢复本地任务事务", &error))?;
        for task_id in &ids {
            sqlx::query(
                "UPDATE local_task SET state = 'interrupted', error_code = 'APP_INTERRUPTED', \
                 message = '应用上次运行未安全结束，需要对账', ended_at = ?, \
                 sequence = sequence + 1, updated_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(&now)
            .bind(task_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("标记中断本地任务", &error))?;
            sqlx::query(
                "UPDATE local_task_target SET state = CASE state WHEN 'running' THEN 'interrupted' \
                 ELSE 'unknown' END, updated_at = ? WHERE local_task_id = ? \
                 AND state IN ('pending', 'running')",
            )
            .bind(&now)
            .bind(task_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("标记中断任务目标", &error))?;
            sqlx::query(
                "UPDATE local_task_step SET state = 'interrupted', error_code = 'APP_INTERRUPTED', \
                 ended_at = ?, updated_at = ? WHERE local_task_id = ? AND state = 'running'",
            )
            .bind(&now)
            .bind(&now)
            .bind(task_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("标记中断任务步骤", &error))?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交恢复本地任务事务", &error))?;
        Ok(ids)
    }

    pub async fn reserve_sequence(&self, task_id: &str) -> AppResult<u64> {
        let now = timestamp();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| AppError::database("开始分配任务事件序号事务", &error))?;
        let sequence = next_sequence(&mut transaction, task_id, &now).await?;
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交任务事件序号事务", &error))?;
        Ok(sequence)
    }

    pub async fn delete_test_task(&self, task_id: &str) -> AppResult<()> {
        sqlx::query("DELETE FROM local_task WHERE id = ?")
            .bind(task_id)
            .execute(&self.pool)
            .await
            .map_err(|error| AppError::database("精确清理测试任务", &error))?;
        Ok(())
    }
}

async fn next_sequence(
    transaction: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    now: &str,
) -> AppResult<u64> {
    let sequence = sqlx::query_scalar::<_, i64>(
        "UPDATE local_task SET sequence = sequence + 1, updated_at = ? WHERE id = ? \
         RETURNING sequence",
    )
    .bind(now)
    .bind(task_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| AppError::database("递增任务事件序号", &error))?
    .ok_or_else(|| AppError::NotFound(format!("本地任务不存在：{task_id}")))?;
    to_u64(sequence, "任务事件序号")
}

fn map_task(row: sqlx::sqlite::SqliteRow) -> AppResult<TaskRecord> {
    Ok(TaskRecord {
        id: get(&row, "id", "任务ID")?,
        local_project_id: get(&row, "local_project_id", "任务项目")?,
        remote_operation_record_id: get(&row, "remote_operation_record_id", "远端操作ID")?,
        domain_type: get(&row, "domain_type", "任务业务域")?,
        operation_type: get(&row, "operation_type", "任务类型")?,
        name: get(&row, "name", "任务名称")?,
        state: TaskState::parse(&get::<String>(&row, "state", "任务状态")?)?,
        priority: get(&row, "priority", "任务优先级")?,
        batch_size: to_u32(get(&row, "batch_size", "任务批次大小")?, "任务批次大小")?,
        concurrency: to_u32(get(&row, "concurrency", "任务并发数")?, "任务并发数")?,
        payload_ref: get(&row, "payload_ref", "任务负载引用")?,
        sequence: to_u64(get(&row, "sequence", "任务序号")?, "任务序号")?,
        log_path: get(&row, "log_path", "任务日志路径")?,
        error_code: get(&row, "error_code", "任务错误码")?,
        message: get(&row, "message", "任务消息")?,
        created_at: get(&row, "created_at", "任务创建时间")?,
        started_at: get(&row, "started_at", "任务开始时间")?,
        ended_at: get(&row, "ended_at", "任务结束时间")?,
        updated_at: get(&row, "updated_at", "任务更新时间")?,
    })
}

fn map_target(row: sqlx::sqlite::SqliteRow) -> AppResult<TaskTargetRecord> {
    TaskTargetRecord::try_from(TargetRow {
        task_id: get(&row, "local_task_id", "目标任务ID")?,
        resource_type: get(&row, "resource_type", "目标资源类型")?,
        resource_key: get(&row, "resource_key", "目标资源键")?,
        state: get(&row, "state", "目标状态")?,
        stage: get(&row, "stage", "目标阶段")?,
        progress_current: get(&row, "progress_current", "目标当前进度")?,
        progress_total: get(&row, "progress_total", "目标总进度")?,
        fencing_token: get(&row, "fencing_token", "目标fencing token")?,
        message_code: get(&row, "message_code", "目标消息码")?,
        message_params_json: get(&row, "message_params_json", "目标消息参数")?,
        updated_at: get(&row, "updated_at", "目标更新时间")?,
    })
}

struct TargetRow {
    task_id: String,
    resource_type: String,
    resource_key: String,
    state: String,
    stage: String,
    progress_current: i64,
    progress_total: i64,
    fencing_token: Option<i64>,
    message_code: Option<String>,
    message_params_json: Option<String>,
    updated_at: String,
}

impl TryFrom<TargetRow> for TaskTargetRecord {
    type Error = AppError;

    fn try_from(value: TargetRow) -> Result<Self, Self::Error> {
        Ok(Self {
            local_task_id: value.task_id,
            resource_type: value.resource_type,
            resource_key: value.resource_key,
            state: TargetState::parse(&value.state)?,
            stage: value.stage,
            progress_current: to_u64(value.progress_current, "目标当前进度")?,
            progress_total: to_u64(value.progress_total, "目标总进度")?,
            fencing_token: value
                .fencing_token
                .map(|item| to_u64(item, "目标fencing token"))
                .transpose()?,
            message_code: value.message_code,
            message_params_json: value.message_params_json,
            updated_at: value.updated_at,
        })
    }
}

fn map_step(row: sqlx::sqlite::SqliteRow) -> AppResult<TaskStepRecord> {
    let state: String = get(&row, "state", "步骤状态")?;
    let state = match state.as_str() {
        "pending" => StepState::Pending,
        "running" => StepState::Running,
        "succeeded" => StepState::Succeeded,
        "failed" => StepState::Failed,
        "skipped" => StepState::Skipped,
        "cancelled" => StepState::Cancelled,
        "interrupted" => StepState::Interrupted,
        _ => return Err(AppError::InvalidConfig(format!("未知步骤状态：{state}"))),
    };
    Ok(TaskStepRecord {
        id: get(&row, "id", "步骤ID")?,
        local_task_id: get(&row, "local_task_id", "步骤任务ID")?,
        resource_type: get(&row, "resource_type", "步骤资源类型")?,
        resource_key: get(&row, "resource_key", "步骤资源键")?,
        step_code: get(&row, "step_code", "步骤码")?,
        state,
        error_code: get(&row, "error_code", "步骤错误码")?,
        message: get(&row, "message", "步骤消息")?,
        started_at: get(&row, "started_at", "步骤开始时间")?,
        ended_at: get(&row, "ended_at", "步骤结束时间")?,
        updated_at: get(&row, "updated_at", "步骤更新时间")?,
    })
}

fn get<T>(
    row: &sqlx::sqlite::SqliteRow,
    column: &'static str,
    operation: &'static str,
) -> AppResult<T>
where
    T: for<'r> sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite>,
{
    row.try_get(column)
        .map_err(|error| AppError::database(operation, &error))
}

fn to_u64(value: i64, field: &'static str) -> AppResult<u64> {
    u64::try_from(value).map_err(|_| AppError::InvalidConfig(format!("{field}超出范围")))
}

fn to_u32(value: i64, field: &'static str) -> AppResult<u32> {
    u32::try_from(value).map_err(|_| AppError::InvalidConfig(format!("{field}超出范围")))
}

fn to_i64(value: u64, field: &'static str) -> AppResult<i64> {
    i64::try_from(value).map_err(|_| AppError::InvalidConfig(format!("{field}超出范围")))
}

fn timestamp() -> String {
    OffsetDateTime::now_utc().unix_timestamp_nanos().to_string()
}
