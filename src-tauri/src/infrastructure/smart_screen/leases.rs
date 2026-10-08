use super::write_context::ScreenWriteContext;
use crate::core::error::{AppError, AppResult};
use crate::formal::resource_lease_repository::{LeaseGrant, LeaseRequest, ResourceLeaseRepository};
use crate::infrastructure::project_context::map_formal_error;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub struct HeldScreenLeases {
    pub grants: Vec<LeaseGrant>,
    repository: ResourceLeaseRepository,
    stop: CancellationToken,
    pub lost: CancellationToken,
    heartbeat: Option<tokio::task::JoinHandle<()>>,
}
impl HeldScreenLeases {
    pub async fn acquire(
        context: &ScreenWriteContext,
        operation: &str,
        instance: &str,
        screen_ids: &[String],
        registry: bool,
        recovery: bool,
    ) -> AppResult<Self> {
        // 与人工释放串行，防止原任务在释放后重新拿锁继续写入。
        let mut operation_guard = context.shared.begin().await.map_err(|e| AppError::database("核对屏操作", &e))?;
        super::lock_release::lock_operation(&mut operation_guard, &context.business, operation).await?;
        super::lock_release::ensure_not_released(&mut operation_guard, operation).await?;
        let repository = ResourceLeaseRepository::new(context.shared.clone());
        let mut keys: Vec<(String, String)> = screen_ids
            .iter()
            .map(|id| ("smart_screen".into(), format!("{}:{id}", context.business)))
            .collect();
        if registry {
            keys.push(("smart_screen_registry".into(), context.source.clone()));
        }
        // 到期只说明工作台失联，不代表设备命令或平台事务已经结束。
        for (kind, key) in &keys {
            let previous:Option<(String,String)>=sqlx::query_as("SELECT operation_id,owner_instance_id FROM resource_lease WHERE resource_type=? AND resource_key=? AND lease_state='active' AND expires_at<=UTC_TIMESTAMP(6)")
                .bind(kind).bind(key).fetch_optional(&context.shared).await.map_err(|e|AppError::database("核对上次屏操作",&e))?;
            if previous
                .as_ref()
                .is_some_and(|(id, owner)| id != operation || owner != instance)
            {
                return Err(AppError::Conflict(
                    "上次操作占用已到期，结果尚未核实；可到项目操作记录查看并确认释放占用".into(),
                ));
            }
        }
        let requests = keys
            .into_iter()
            .map(|(resource_type, resource_key)| LeaseRequest {
                resource_type,
                resource_key,
                domain_type: "smart_screen".into(),
                operation_id: operation.into(),
                owner_instance_id: instance.into(),
                owner_user: context.operator.clone(),
                ttl: Duration::from_secs(90),
            })
            .collect();
        let grants = if recovery {
            repository.recover_same_operation(requests).await
        } else {
            repository.acquire_many(requests).await
        }
        .map_err(map_formal_error)?;
        operation_guard.commit().await.map_err(|e| AppError::database("确认屏操作占用", &e))?;
        let stop = CancellationToken::new();
        let lost = CancellationToken::new();
        let (worker_stop, worker_lost, worker_repo, worker_grants) = (
            stop.clone(),
            lost.clone(),
            repository.clone(),
            grants.clone(),
        );
        let heartbeat_pool = context.shared.clone();
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::select! {_=worker_stop.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(20))=>{}}
                for grant in &worker_grants {
                    let result = tokio::select! {_=worker_stop.cancelled()=>return,result=tokio::time::timeout(Duration::from_secs(10),worker_repo.heartbeat(grant,Duration::from_secs(90)))=>result};
                    if !matches!(result, Ok(Ok(()))) {
                        worker_lost.cancel();
                        return;
                    }
                    let _=tokio::time::timeout(Duration::from_secs(5),sqlx::query("UPDATE operation_record SET heartbeat_at=UTC_TIMESTAMP(6) WHERE id=? AND state='running'").bind(&grant.operation_id).execute(&heartbeat_pool)).await;
                }
            }
        });
        Ok(Self {
            grants,
            repository,
            stop,
            lost,
            heartbeat: Some(heartbeat),
        })
    }
    pub async fn valid(&self) -> AppResult<()> {
        if self.lost.is_cancelled() {
            return Err(AppError::Conflict(
                "设备操作占用已失效，停止后续操作".into(),
            ));
        }
        for grant in &self.grants {
            if !self
                .repository
                .validate_fencing(grant)
                .await
                .map_err(map_formal_error)?
            {
                return Err(AppError::Conflict(
                    "设备操作已由其他任务接管，停止后续操作".into(),
                ));
            }
        }
        Ok(())
    }
    pub async fn release(mut self) -> AppResult<()> {
        self.stop.cancel();
        if let Some(worker) = self.heartbeat.take() {
            let _ = worker.await;
        }
        let mut first = None;
        for grant in &self.grants {
            if let Err(error) = self.repository.release(grant).await {
                first.get_or_insert(map_formal_error(error));
            }
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
impl Drop for HeldScreenLeases {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
