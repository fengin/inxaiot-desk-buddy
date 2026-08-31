use std::collections::BTreeSet;
use std::time::Duration;

use sqlx::{MySql, MySqlPool, Row, Transaction};
use uuid::Uuid;

use super::error::{FormalError, FormalResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseGrant {
    pub resource_type: String,
    pub resource_key: String,
    pub operation_id: String,
    pub owner_instance_id: String,
    pub lease_token: String,
    pub fencing_token: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LeaseRequest {
    pub resource_type: String,
    pub resource_key: String,
    pub domain_type: String,
    pub operation_id: String,
    pub owner_instance_id: String,
    pub owner_user: String,
    pub ttl: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseOutcome {
    Acquired(LeaseGrant),
    Busy {
        resource_type: String,
        resource_key: String,
        owner_instance_id: String,
        operation_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveLease {
    pub resource_type: String,
    pub resource_key: String,
    pub owner_instance_id: String,
    pub operation_id: String,
}

#[derive(Clone)]
pub struct ResourceLeaseRepository {
    pool: MySqlPool,
}

impl ResourceLeaseRepository {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    pub async fn active_lease(
        &self,
        resource_type: &str,
        resource_key: &str,
    ) -> FormalResult<Option<ActiveLease>> {
        if resource_type.trim().is_empty() || resource_key.trim().is_empty() {
            return Err(FormalError::InvalidConfig("租约查询目标不能为空".into()));
        }
        let row = sqlx::query(concat!(
            "SELECT resource_type, resource_key, owner_instance_id, operation_id ",
            "FROM resource_lease WHERE resource_type = ? AND resource_key = ? ",
            "AND lease_state = 'active' AND expires_at > UTC_TIMESTAMP(6)"
        ))
        .bind(resource_type)
        .bind(resource_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| map_error("查询活动资源租约", error))?;
        row.map(|row| {
            Ok(ActiveLease {
                resource_type: row
                    .try_get("resource_type")
                    .map_err(|_| FormalError::LocalDatabase("解析活动租约资源类型"))?,
                resource_key: row
                    .try_get("resource_key")
                    .map_err(|_| FormalError::LocalDatabase("解析活动租约资源标识"))?,
                owner_instance_id: row
                    .try_get("owner_instance_id")
                    .map_err(|_| FormalError::LocalDatabase("解析活动租约实例"))?,
                operation_id: row
                    .try_get("operation_id")
                    .map_err(|_| FormalError::LocalDatabase("解析活动租约操作"))?,
            })
        })
        .transpose()
    }

    pub async fn acquire_many(&self, requests: Vec<LeaseRequest>) -> FormalResult<Vec<LeaseGrant>> {
        if requests.is_empty() {
            return Err(FormalError::InvalidConfig("租约目标不能为空".into()));
        }
        let unique = requests
            .into_iter()
            .map(|request| {
                (
                    (request.resource_type.clone(), request.resource_key.clone()),
                    request,
                )
            })
            .collect::<BTreeSet<_>>();
        let mut transaction = self.pool.begin().await.map_err(|error| {
            tracing::error!(error = ?error, "begin lease transaction failed");
            FormalError::LocalDatabase("开始资源租约事务")
        })?;
        let mut grants = Vec::with_capacity(unique.len());
        for (_, request) in unique {
            match acquire_one(&mut transaction, &request).await? {
                LeaseOutcome::Acquired(grant) => grants.push(grant),
                LeaseOutcome::Busy {
                    resource_type,
                    resource_key,
                    owner_instance_id,
                    operation_id,
                } => {
                    return Err(FormalError::Conflict(format!(
                        "资源被占用：{resource_type}/{resource_key}，实例={owner_instance_id}，操作={operation_id}"
                    )));
                }
            }
        }
        transaction.commit().await.map_err(|error| {
            tracing::error!(error = ?error, "commit lease transaction failed");
            FormalError::LocalDatabase("提交资源租约事务")
        })?;
        Ok(grants)
    }

    pub async fn heartbeat(&self, grant: &LeaseGrant, ttl: Duration) -> FormalResult<()> {
        let result = sqlx::query(
            "UPDATE resource_lease SET heartbeat_at = UTC_TIMESTAMP(6), \
             expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND) \
             WHERE resource_type = ? AND resource_key = ? AND lease_state = 'active' \
             AND owner_instance_id = ? AND lease_token = ? AND fencing_token = ? \
             AND expires_at > UTC_TIMESTAMP(6)",
        )
        .bind(ttl.as_secs().max(1) as i64)
        .bind(&grant.resource_type)
        .bind(&grant.resource_key)
        .bind(&grant.owner_instance_id)
        .bind(&grant.lease_token)
        .bind(grant.fencing_token)
        .execute(&self.pool)
        .await
        .map_err(|error| map_error("续租资源", error))?;
        if result.rows_affected() != 1 {
            return Err(FormalError::Conflict("资源租约已失效".into()));
        }
        Ok(())
    }

    pub async fn validate_fencing(&self, grant: &LeaseGrant) -> FormalResult<bool> {
        let valid = sqlx::query_scalar::<_, i8>(
            "SELECT EXISTS(SELECT 1 FROM resource_lease WHERE resource_type = ? \
             AND resource_key = ? AND lease_state = 'active' AND owner_instance_id = ? \
             AND lease_token = ? AND fencing_token = ? AND expires_at > UTC_TIMESTAMP(6))",
        )
        .bind(&grant.resource_type)
        .bind(&grant.resource_key)
        .bind(&grant.owner_instance_id)
        .bind(&grant.lease_token)
        .bind(grant.fencing_token)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| map_error("校验fencing token", error))?;
        Ok(valid == 1)
    }

    pub async fn release(&self, grant: &LeaseGrant) -> FormalResult<()> {
        let result = sqlx::query(
            "UPDATE resource_lease SET lease_state = 'released', heartbeat_at = UTC_TIMESTAMP(6), \
             expires_at = UTC_TIMESTAMP(6) WHERE resource_type = ? AND resource_key = ? \
             AND lease_state = 'active' AND owner_instance_id = ? AND lease_token = ? \
             AND fencing_token = ?",
        )
        .bind(&grant.resource_type)
        .bind(&grant.resource_key)
        .bind(&grant.owner_instance_id)
        .bind(&grant.lease_token)
        .bind(grant.fencing_token)
        .execute(&self.pool)
        .await
        .map_err(|error| map_error("释放资源租约", error))?;
        if result.rows_affected() != 1 {
            return Err(FormalError::Conflict("资源租约已被接管或释放".into()));
        }
        Ok(())
    }

    pub async fn delete_test_counter(
        &self,
        resource_type: &str,
        resource_key: &str,
    ) -> FormalResult<()> {
        sqlx::query(
            "DELETE FROM resource_lease WHERE resource_type = ? AND resource_key = ? AND lease_state = 'released'",
        )
        .bind(resource_type)
        .bind(resource_key)
        .execute(&self.pool)
        .await
        .map_err(|error| map_error("清理测试租约计数行", error))?;
        Ok(())
    }
}

async fn acquire_one(
    transaction: &mut Transaction<'_, MySql>,
    request: &LeaseRequest,
) -> FormalResult<LeaseOutcome> {
    validate_request(request)?;
    let row = sqlx::query(
        "SELECT owner_instance_id, operation_id, fencing_token, lease_state, \
         expires_at <= UTC_TIMESTAMP(6) AS expired FROM resource_lease \
         WHERE resource_type = ? AND resource_key = ? FOR UPDATE",
    )
    .bind(&request.resource_type)
    .bind(&request.resource_key)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| map_error("读取资源租约", error))?;
    let lease_token = Uuid::now_v7().to_string();
    let ttl_seconds = request.ttl.as_secs().max(1) as i64;
    if let Some(row) = row {
        let owner: String = row
            .try_get("owner_instance_id")
            .map_err(|_| FormalError::LocalDatabase("解析租约实例"))?;
        let operation_id: String = row
            .try_get("operation_id")
            .map_err(|_| FormalError::LocalDatabase("解析租约操作"))?;
        let token: u64 = row
            .try_get("fencing_token")
            .map_err(|_| FormalError::LocalDatabase("解析fencing token"))?;
        let state: String = row
            .try_get("lease_state")
            .map_err(|_| FormalError::LocalDatabase("解析租约状态"))?;
        let expired: i8 = row
            .try_get("expired")
            .map_err(|_| FormalError::LocalDatabase("解析租约过期状态"))?;
        if state == "active" && expired == 0 {
            return Ok(LeaseOutcome::Busy {
                resource_type: request.resource_type.clone(),
                resource_key: request.resource_key.clone(),
                owner_instance_id: owner,
                operation_id,
            });
        }
        let next_fencing = token + 1;
        sqlx::query(
            "UPDATE resource_lease SET domain_type = ?, operation_id = ?, owner_instance_id = ?, \
             owner_user = ?, lease_token = ?, fencing_token = ?, lease_state = 'active', \
             acquired_at = UTC_TIMESTAMP(6), heartbeat_at = UTC_TIMESTAMP(6), \
             expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND) \
             WHERE resource_type = ? AND resource_key = ?",
        )
        .bind(&request.domain_type)
        .bind(&request.operation_id)
        .bind(&request.owner_instance_id)
        .bind(&request.owner_user)
        .bind(&lease_token)
        .bind(next_fencing)
        .bind(ttl_seconds)
        .bind(&request.resource_type)
        .bind(&request.resource_key)
        .execute(&mut **transaction)
        .await
        .map_err(|error| map_error("接管资源租约", error))?;
        Ok(LeaseOutcome::Acquired(grant(
            request,
            lease_token,
            next_fencing,
        )))
    } else {
        sqlx::query(
            "INSERT INTO resource_lease \
             (resource_type, resource_key, domain_type, operation_id, owner_instance_id, owner_user, \
              lease_token, fencing_token, lease_state, acquired_at, heartbeat_at, expires_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 1, 'active', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), \
              DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND))",
        )
        .bind(&request.resource_type)
        .bind(&request.resource_key)
        .bind(&request.domain_type)
        .bind(&request.operation_id)
        .bind(&request.owner_instance_id)
        .bind(&request.owner_user)
        .bind(&lease_token)
        .bind(ttl_seconds)
        .execute(&mut **transaction)
        .await
        .map_err(|error| map_error("创建资源租约", error))?;
        Ok(LeaseOutcome::Acquired(grant(request, lease_token, 1)))
    }
}

fn grant(request: &LeaseRequest, lease_token: String, fencing_token: u64) -> LeaseGrant {
    LeaseGrant {
        resource_type: request.resource_type.clone(),
        resource_key: request.resource_key.clone(),
        operation_id: request.operation_id.clone(),
        owner_instance_id: request.owner_instance_id.clone(),
        lease_token,
        fencing_token,
    }
}

fn validate_request(request: &LeaseRequest) -> FormalResult<()> {
    if request.resource_type.is_empty()
        || request.resource_key.is_empty()
        || request.domain_type.is_empty()
        || request.operation_id.is_empty()
        || request.owner_instance_id.is_empty()
        || request.owner_user.is_empty()
        || request.ttl.is_zero()
    {
        return Err(FormalError::InvalidConfig("资源租约参数不完整".into()));
    }
    Ok(())
}

fn map_error(operation: &'static str, error: sqlx::Error) -> FormalError {
    tracing::error!(operation, error = ?error, "resource lease mysql operation failed");
    FormalError::LocalDatabase(operation)
}
