use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::MySqlPool;

use crate::core::error::{AppError, AppResult};
use crate::domain::common::task::TaskState;
use crate::formal::aio_node_repository::ServiceVersionWrite;
use crate::formal::operation_repository::{OperationFinalResult, TargetFinalResult};
use crate::formal::resource_lease_repository::LeaseGrant;
use crate::infrastructure::local_sqlite::task_repository::{TargetUpdate, TaskStepWrite};

const PENDING_LOCAL_FINALIZATION_FILE: &str = "pending-local-finalization.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingLocalFinalization {
    pub operation_id: String,
    pub final_state: TaskState,
    pub targets: Vec<TargetUpdate>,
    pub steps: Vec<TaskStepWrite>,
}

pub fn write_pending_local_finalization(
    task_dir: &Path,
    projection: &PendingLocalFinalization,
) -> AppResult<PathBuf> {
    if projection.operation_id.trim().is_empty()
        || !projection.final_state.is_terminal()
        || projection.targets.is_empty()
    {
        return Err(AppError::InvalidConfig("待重试本地最终化记录无效".into()));
    }
    std::fs::create_dir_all(task_dir)
        .map_err(|error| AppError::io("创建最终化重试目录", &error))?;
    let path = task_dir.join(PENDING_LOCAL_FINALIZATION_FILE);
    let temporary = task_dir.join(format!(
        "{PENDING_LOCAL_FINALIZATION_FILE}.{}.tmp",
        std::process::id()
    ));
    let bytes = serde_json::to_vec(projection)
        .map_err(|_| AppError::InvalidConfig("序列化本地最终化重试记录失败".into()))?;
    std::fs::write(&temporary, bytes)
        .map_err(|error| AppError::io("写入本地最终化重试记录", &error))?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| AppError::io("发布本地最终化重试记录", &error))?;
    Ok(path)
}

pub fn read_pending_local_finalization(task_dir: &Path) -> AppResult<PendingLocalFinalization> {
    let path = task_dir.join(PENDING_LOCAL_FINALIZATION_FILE);
    let bytes =
        std::fs::read(&path).map_err(|error| AppError::io("读取本地最终化重试记录", &error))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::InvalidConfig("本地最终化重试记录无法解析".into()))
}

pub async fn shared_operation_state(
    pool: &MySqlPool,
    operation_id: &str,
) -> AppResult<Option<String>> {
    sqlx::query_scalar::<_, String>("SELECT state FROM operation_record WHERE id = ?")
        .bind(operation_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::database("读取共享最终化操作状态", &error))
}

#[derive(Clone, Debug)]
pub struct AtomicTargetFinalization {
    pub result: TargetFinalResult,
    pub service_versions: Vec<ServiceVersionWrite>,
    pub mark_operation_success: bool,
}

#[derive(Clone, Debug)]
pub struct AtomicDeploymentFinalization {
    pub operation: OperationFinalResult,
    pub targets: Vec<AtomicTargetFinalization>,
    pub leases: Vec<LeaseGrant>,
}

pub async fn finalize_deployment_atomically(
    pool: &MySqlPool,
    write: AtomicDeploymentFinalization,
) -> AppResult<()> {
    validate_write(&write)?;
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| AppError::database("开始部署最终化事务", &error))?;

    for lease in &write.leases {
        let fencing = sqlx::query_scalar::<_, u64>(concat!(
            "SELECT fencing_token FROM resource_lease WHERE resource_type = ? ",
            "AND resource_key = ? AND operation_id = ? AND owner_instance_id = ? ",
            "AND lease_token = ? AND lease_state = 'active' ",
            "AND expires_at > UTC_TIMESTAMP(6) FOR UPDATE"
        ))
        .bind(&lease.resource_type)
        .bind(&lease.resource_key)
        .bind(&lease.operation_id)
        .bind(&lease.owner_instance_id)
        .bind(&lease.lease_token)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| AppError::database("锁定部署fencing租约", &error))?;
        if fencing != Some(lease.fencing_token) {
            return Err(AppError::Conflict(format!(
                "资源租约已失效，禁止最终化：{}/{}",
                lease.resource_type, lease.resource_key
            )));
        }
    }

    let mut success_count = 0_u32;
    let mut failure_count = 0_u32;
    let mut cancelled_count = 0_u32;
    for target in &write.targets {
        match target.result.result_state.as_str() {
            "succeeded" => success_count += 1,
            "failed" | "interrupted" | "unknown" => failure_count += 1,
            "cancelled" => cancelled_count += 1,
            _ => {
                return Err(AppError::InvalidConfig("部署目标最终状态无效".into()));
            }
        }
        let updated = sqlx::query(concat!(
            "UPDATE operation_target_result SET result_state = ?, before_version = ?, ",
            "after_version = ?, result_summary = ?, error_code = ?, error_summary = ?, ",
            "completed_at = UTC_TIMESTAMP(6) WHERE operation_id = ? AND resource_type = ? ",
            "AND resource_key = ? AND result_state = 'pending'"
        ))
        .bind(&target.result.result_state)
        .bind(&target.result.before_version)
        .bind(&target.result.after_version)
        .bind(&target.result.result_summary)
        .bind(&target.result.error_code)
        .bind(&target.result.error_summary)
        .bind(&target.result.operation_id)
        .bind(&target.result.resource_type)
        .bind(&target.result.resource_key)
        .execute(&mut *transaction)
        .await
        .map_err(|error| AppError::database("写入部署目标最终结果", &error))?;
        if updated.rows_affected() != 1 {
            return Err(AppError::Conflict(format!(
                "部署目标已最终化或不存在：{}",
                target.result.resource_key
            )));
        }

        for service in &target.service_versions {
            sqlx::query(concat!(
                "INSERT INTO aio_node_service_version ",
                "(mac_normalized, service_name, expected_image_name, expected_version, ",
                "observed_image_name, observed_version, observed_at, source_operation_id) ",
                "VALUES (?, ?, ?, ?, ?, ?, UTC_TIMESTAMP(6), ?) ",
                "ON DUPLICATE KEY UPDATE expected_image_name = VALUES(expected_image_name), ",
                "expected_version = VALUES(expected_version), ",
                "observed_image_name = VALUES(observed_image_name), ",
                "observed_version = VALUES(observed_version), observed_at = VALUES(observed_at), ",
                "source_operation_id = VALUES(source_operation_id)"
            ))
            .bind(&service.mac)
            .bind(service.service_name.trim())
            .bind(&service.expected_image_name)
            .bind(&service.expected_version)
            .bind(&service.observed_image_name)
            .bind(&service.observed_version)
            .bind(&service.source_operation_id)
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("写入部署服务版本", &error))?;
        }
        if target.mark_operation_success {
            let updated = sqlx::query(concat!(
                "UPDATE aio_node SET last_operation_id = ?, management_state = 'managed', ",
                "version = version + 1, updated_at = UTC_TIMESTAMP(6) WHERE mac_normalized = ?"
            ))
            .bind(&write.operation.operation_id)
            .bind(&target.result.resource_key)
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("更新一体机最终资产", &error))?;
            if updated.rows_affected() != 1 {
                return Err(AppError::NotFound(format!(
                    "一体机最终资产不存在：{}",
                    target.result.resource_key
                )));
            }
        }
    }

    let updated = sqlx::query(concat!(
        "UPDATE operation_record SET state = ?, success_count = ?, failure_count = ?, ",
        "cancelled_count = ?, result_summary = ?, error_code = ?, error_summary = ?, ",
        "ended_at = UTC_TIMESTAMP(6), heartbeat_at = UTC_TIMESTAMP(6), version = version + 1 ",
        "WHERE id = ? AND state = 'running' AND version = ?"
    ))
    .bind(&write.operation.state)
    .bind(success_count)
    .bind(failure_count)
    .bind(cancelled_count)
    .bind(&write.operation.result_summary)
    .bind(&write.operation.error_code)
    .bind(&write.operation.error_summary)
    .bind(&write.operation.operation_id)
    .bind(write.operation.expected_version)
    .execute(&mut *transaction)
    .await
    .map_err(|error| AppError::database("最终化部署操作", &error))?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "部署操作版本或状态已变化，最终化已回滚".into(),
        ));
    }

    for lease in &write.leases {
        let released = sqlx::query(concat!(
            "UPDATE resource_lease SET lease_state = 'released', heartbeat_at = UTC_TIMESTAMP(6), ",
            "expires_at = UTC_TIMESTAMP(6) WHERE resource_type = ? AND resource_key = ? ",
            "AND operation_id = ? AND lease_state = 'active' AND owner_instance_id = ? ",
            "AND lease_token = ? AND fencing_token = ?"
        ))
        .bind(&lease.resource_type)
        .bind(&lease.resource_key)
        .bind(&lease.operation_id)
        .bind(&lease.owner_instance_id)
        .bind(&lease.lease_token)
        .bind(lease.fencing_token)
        .execute(&mut *transaction)
        .await
        .map_err(|error| AppError::database("释放部署最终化租约", &error))?;
        if released.rows_affected() != 1 {
            return Err(AppError::Conflict(format!(
                "部署租约释放失败，最终化已回滚：{}/{}",
                lease.resource_type, lease.resource_key
            )));
        }
    }

    transaction
        .commit()
        .await
        .map_err(|error| AppError::database("提交部署最终化事务", &error))
}

fn validate_write(write: &AtomicDeploymentFinalization) -> AppResult<()> {
    if write.operation.operation_id.trim().is_empty()
        || write.targets.is_empty()
        || write.leases.len() != write.targets.len()
    {
        return Err(AppError::InvalidConfig("部署最终化参数不完整".into()));
    }
    let target_keys = write
        .targets
        .iter()
        .map(|target| target.result.resource_key.as_str())
        .collect::<BTreeSet<_>>();
    let lease_keys = write
        .leases
        .iter()
        .map(|lease| lease.resource_key.as_str())
        .collect::<BTreeSet<_>>();
    if target_keys.len() != write.targets.len()
        || lease_keys.len() != write.leases.len()
        || target_keys != lease_keys
        || write.targets.iter().any(|target| {
            target.result.operation_id != write.operation.operation_id
                || target.result.resource_type != "aio"
                || target.service_versions.iter().any(|service| {
                    service.mac != target.result.resource_key
                        || service.service_name.trim().is_empty()
                        || service.source_operation_id.as_deref()
                            != Some(write.operation.operation_id.as_str())
                })
        })
        || write
            .leases
            .iter()
            .any(|lease| lease.operation_id != write.operation.operation_id)
    {
        return Err(AppError::InvalidConfig(
            "部署最终化目标、服务版本与租约不一致".into(),
        ));
    }
    Ok(())
}
