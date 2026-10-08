use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, Row};

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
    pub shared: AtomicDeploymentFinalization,
}

pub fn write_pending_local_finalization(
    task_dir: &Path,
    projection: &PendingLocalFinalization,
) -> AppResult<PathBuf> {
    if projection.operation_id.trim().is_empty()
        || !projection.final_state.is_terminal()
        || projection.targets.is_empty()
        || projection.shared.operation.operation_id != projection.operation_id
        || projection.shared.operation.state != projection.final_state.as_str()
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtomicTargetFinalization {
    #[serde(default)]
    pub asset: Option<crate::domain::aio::inventory::WorkbenchNodeSnapshot>,
    pub result: TargetFinalResult,
    pub service_versions: Vec<ServiceVersionWrite>,
    pub replace_service_versions: bool,
    pub mark_operation_success: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AtomicDeploymentFinalization {
    pub operation: OperationFinalResult,
    pub targets: Vec<AtomicTargetFinalization>,
    pub leases: Vec<LeaseGrant>,
}

pub use crate::domain::common::task::TaskRecoveryConflict as FinalizationLeaseConflict;

pub async fn finalization_lease_conflicts(
    pool: &MySqlPool,
    write: &AtomicDeploymentFinalization,
) -> AppResult<Vec<FinalizationLeaseConflict>> {
    let mut conflicts = Vec::new();
    for expected in &write.leases {
        let row = sqlx::query(concat!(
            "SELECT owner_instance_id, operation_id, lease_token, fencing_token, lease_state ",
            "FROM resource_lease WHERE resource_type = ? AND resource_key = ?"
        ))
        .bind(&expected.resource_type)
        .bind(&expected.resource_key)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::database("读取最终化租约占用", &error))?;
        let Some(row) = row else {
            conflicts.push(FinalizationLeaseConflict {
                resource_key: expected.resource_key.clone(),
                owner_instance_id: "未知实例".into(),
                operation_id: "未知任务".into(),
                takeover_allowed: false,
            });
            continue;
        };
        let owner_instance_id = row
            .try_get::<String, _>("owner_instance_id")
            .map_err(|error| AppError::database("解析最终化租约实例", &error))?;
        let operation_id = row
            .try_get::<String, _>("operation_id")
            .map_err(|error| AppError::database("解析最终化租约任务", &error))?;
        let lease_token = row
            .try_get::<String, _>("lease_token")
            .map_err(|error| AppError::database("解析最终化租约令牌", &error))?;
        let fencing_token = row
            .try_get::<u64, _>("fencing_token")
            .map_err(|error| AppError::database("解析最终化fencing令牌", &error))?;
        let lease_state = row
            .try_get::<String, _>("lease_state")
            .map_err(|error| AppError::database("解析最终化租约状态", &error))?;
        if lease_state != "active"
            || owner_instance_id != expected.owner_instance_id
            || operation_id != expected.operation_id
            || lease_token != expected.lease_token
            || fencing_token != expected.fencing_token
        {
            let operation_state = shared_operation_state(pool, &operation_id).await?;
            conflicts.push(FinalizationLeaseConflict {
                resource_key: expected.resource_key.clone(),
                owner_instance_id,
                operation_id,
                takeover_allowed: lease_state == "active"
                    && operation_state.as_deref() == Some("running"),
            });
        }
    }
    Ok(conflicts)
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
            "AND lease_token = ? AND lease_state = 'active' FOR UPDATE"
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
        if let Some(asset) = &target.asset {
            // 只有部署成功的设备才建立结果关联；新增/导入清单不进入共享资产表。
            sqlx::query("INSERT INTO aio_node(mac_normalized,name,ip,display_mac,building_id,addr_alias,location,remark,platform_aio_id,region_id,floor,management_state,source,version,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,'managed','deployment',1,UTC_TIMESTAMP(6),UTC_TIMESTAMP(6)) ON DUPLICATE KEY UPDATE platform_aio_id=COALESCE(VALUES(platform_aio_id),platform_aio_id)")
                .bind(&asset.mac_normalized).bind(&asset.name).bind(&asset.ip)
                .bind(crate::domain::aio::mac::MacAddress::parse(&asset.mac_normalized)?.display())
                .bind(&asset.building_id).bind(&asset.addr_alias).bind(&asset.addr_alias).bind(&asset.remark).bind(&asset.platform_aio_id)
                .bind(&asset.region_id).bind(&asset.floor)
                .execute(&mut *transaction).await.map_err(|error| AppError::database("保存部署成功的一体机结果关联", &error))?;
        }
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

        if target.replace_service_versions {
            sqlx::query("DELETE FROM aio_node_service_version WHERE mac_normalized = ?")
                .bind(&target.result.resource_key)
                .execute(&mut *transaction)
                .await
                .map_err(|error| AppError::database("替换整包部署服务版本", &error))?;
        }
        for service in &target.service_versions {
            sqlx::query(concat!(
                "INSERT INTO aio_node_service_version ",
                "(mac_normalized, service_name, expected_image_name, expected_version, ",
                "observed_image_name, observed_version, observed_at, source_operation_id) ",
                "VALUES (?, ?, ?, ?, ?, ?, IF(? IS NULL, NULL, UTC_TIMESTAMP(6)), ?) ",
                "ON DUPLICATE KEY UPDATE expected_image_name = VALUES(expected_image_name), ",
                "expected_version = VALUES(expected_version), ",
                "observed_image_name = COALESCE(VALUES(observed_image_name), observed_image_name), ",
                "observed_version = COALESCE(VALUES(observed_version), observed_version), ",
                "observed_at = COALESCE(VALUES(observed_at), observed_at), ",
                "source_operation_id = VALUES(source_operation_id)"
            ))
            .bind(&service.mac)
            .bind(service.service_name.trim())
            .bind(&service.expected_image_name)
            .bind(&service.expected_version)
            .bind(&service.observed_image_name)
            .bind(&service.observed_version)
            .bind(&service.observed_image_name)
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
                || target.asset.as_ref().is_some_and(|asset| asset.mac_normalized != target.result.resource_key || !target.mark_operation_success || target.result.result_state != "succeeded")
                || target.result.resource_type != "aio"
                || (target.replace_service_versions
                    && (!target.mark_operation_success
                        || target.result.result_state != "succeeded"
                        || target.service_versions.is_empty()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::common::task::{StepState, TargetState};

    #[test]
    fn pending_file_keeps_everything_required_for_shared_retry() {
        let directory = tempfile::tempdir().expect("temporary finalization directory");
        let operation_id = "operation-retry".to_string();
        let resource_key = "02AABBCCDDEE".to_string();
        let projection = PendingLocalFinalization {
            operation_id: operation_id.clone(),
            final_state: TaskState::Succeeded,
            targets: vec![TargetUpdate {
                resource_type: "aio".into(),
                resource_key: resource_key.clone(),
                state: TargetState::Succeeded,
                stage: "completed".into(),
                progress_current: 100,
                progress_total: 100,
                fencing_token: Some(7),
                message_code: None,
                message_params_json: None,
            }],
            steps: vec![TaskStepWrite {
                id: "step-retry".into(),
                resource_type: Some("aio".into()),
                resource_key: Some(resource_key.clone()),
                step_code: "health".into(),
                state: StepState::Succeeded,
                error_code: None,
                message: Some("服务健康".into()),
            }],
            shared: AtomicDeploymentFinalization {
                operation: OperationFinalResult {
                    operation_id: operation_id.clone(),
                    expected_version: 3,
                    state: "succeeded".into(),
                    result_summary: Some("成功1，失败0，取消0".into()),
                    error_code: None,
                    error_summary: None,
                },
                targets: vec![AtomicTargetFinalization {
                    asset: None,
                    result: TargetFinalResult {
                        operation_id: operation_id.clone(),
                        resource_type: "aio".into(),
                        resource_key: resource_key.clone(),
                        result_state: "succeeded".into(),
                        before_version: Some("before".into()),
                        after_version: Some("after".into()),
                        result_summary: Some("部署成功".into()),
                        error_code: None,
                        error_summary: None,
                    },
                    service_versions: vec![ServiceVersionWrite {
                        mac: resource_key.clone(),
                        service_name: "device-edge".into(),
                        expected_image_name: Some("device-edge:1.2.3".into()),
                        expected_version: Some("1.2.3".into()),
                        observed_image_name: Some("device-edge:1.2.3".into()),
                        observed_version: Some("1.2.3".into()),
                        source_operation_id: Some(operation_id.clone()),
                    }],
                    replace_service_versions: true,
                    mark_operation_success: true,
                }],
                leases: vec![LeaseGrant {
                    resource_type: "aio".into(),
                    resource_key: resource_key.clone(),
                    operation_id: operation_id.clone(),
                    owner_instance_id: "instance-a".into(),
                    lease_token: "lease-token".into(),
                    fencing_token: 7,
                }],
            },
        };

        write_pending_local_finalization(directory.path(), &projection)
            .expect("write pending file");
        let restored =
            read_pending_local_finalization(directory.path()).expect("read pending file");

        assert_eq!(restored.operation_id, operation_id);
        assert_eq!(restored.shared.operation.expected_version, 3);
        assert_eq!(restored.shared.targets[0].service_versions.len(), 1);
        assert_eq!(restored.shared.leases[0].fencing_token, 7);
        assert_eq!(restored.targets[0].resource_key, resource_key);
    }
}
