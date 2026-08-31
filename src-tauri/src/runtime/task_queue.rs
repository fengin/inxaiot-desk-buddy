use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, TryAcquireError, broadcast};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::core::error::{AppError, AppResult};
use crate::runtime::job_supervisor::{JobOutcome, JobSupervisor};

pub type TaskFuture = Pin<Box<dyn Future<Output = AppResult<()>> + Send + 'static>>;
pub type TaskHandler = Arc<dyn Fn(TaskEnvelope, CancellationToken) -> TaskFuture + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskEnvelope {
    pub local_task_id: String,
    pub local_project_id: String,
    pub domain_type: String,
    pub operation_type: String,
    pub resource_keys: Vec<String>,
    pub priority: i32,
    pub payload_ref: Option<String>,
}

impl TaskEnvelope {
    pub fn validate(&self) -> AppResult<()> {
        if self.local_task_id.trim().is_empty()
            || self.local_project_id.trim().is_empty()
            || self.domain_type.trim().is_empty()
            || self.operation_type.trim().is_empty()
            || self.resource_keys.is_empty()
            || self.resource_keys.iter().any(|key| key.trim().is_empty())
        {
            return Err(AppError::InvalidConfig("任务队列信封参数无效".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskQueueResult {
    pub local_task_id: String,
    pub outcome: JobOutcome,
}

#[derive(Clone, Default)]
pub struct TaskHandlerRegistry {
    handlers: Arc<RwLock<HashMap<(String, String), TaskHandler>>>,
}

impl TaskHandlerRegistry {
    pub fn register<F, Fut>(
        &self,
        domain_type: &str,
        operation_type: &str,
        handler: F,
    ) -> AppResult<()>
    where
        F: Fn(TaskEnvelope, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AppResult<()>> + Send + 'static,
    {
        if domain_type.trim().is_empty() || operation_type.trim().is_empty() {
            return Err(AppError::InvalidConfig("任务处理器键不能为空".into()));
        }
        let key = (domain_type.into(), operation_type.into());
        let mut handlers = self
            .handlers
            .write()
            .map_err(|_| AppError::Conflict("任务处理器注册表已损坏".into()))?;
        if handlers.contains_key(&key) {
            return Err(AppError::Conflict(format!(
                "任务处理器已注册：{domain_type}/{operation_type}"
            )));
        }
        handlers.insert(
            key,
            Arc::new(move |envelope, cancellation| Box::pin(handler(envelope, cancellation))),
        );
        Ok(())
    }

    fn get(&self, domain_type: &str, operation_type: &str) -> AppResult<TaskHandler> {
        self.handlers
            .read()
            .map_err(|_| AppError::Conflict("任务处理器注册表已损坏".into()))?
            .get(&(domain_type.into(), operation_type.into()))
            .cloned()
            .ok_or_else(|| {
                AppError::NotFound(format!("任务处理器不存在：{domain_type}/{operation_type}"))
            })
    }

    pub fn contains(&self, domain_type: &str, operation_type: &str) -> AppResult<bool> {
        Ok(self
            .handlers
            .read()
            .map_err(|_| AppError::Conflict("任务处理器注册表已损坏".into()))?
            .contains_key(&(domain_type.into(), operation_type.into())))
    }

    pub fn registered_keys(&self) -> AppResult<Vec<(String, String)>> {
        let mut keys = self
            .handlers
            .read()
            .map_err(|_| AppError::Conflict("任务处理器注册表已损坏".into()))?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        Ok(keys)
    }
}

struct QueuedTask {
    sequence: u64,
    envelope: TaskEnvelope,
    _slot: OwnedSemaphorePermit,
}

impl PartialEq for QueuedTask {
    fn eq(&self, other: &Self) -> bool {
        self.envelope.priority == other.envelope.priority && self.sequence == other.sequence
    }
}

impl Eq for QueuedTask {}

impl PartialOrd for QueuedTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueuedTask {
    fn cmp(&self, other: &Self) -> Ordering {
        self.envelope
            .priority
            .cmp(&other.envelope.priority)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

struct QueueState {
    heap: Mutex<BinaryHeap<QueuedTask>>,
    queued_ids: Mutex<HashSet<String>>,
    items: Semaphore,
    slots: Arc<Semaphore>,
    sequence: AtomicU64,
    closed: AtomicBool,
    shutdown: CancellationToken,
}

#[derive(Clone)]
pub struct TaskQueue {
    state: Arc<QueueState>,
    registry: TaskHandlerRegistry,
    supervisor: JobSupervisor,
    results: broadcast::Sender<TaskQueueResult>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl TaskQueue {
    pub async fn start(
        capacity: usize,
        worker_count: usize,
        registry: TaskHandlerRegistry,
        supervisor: JobSupervisor,
    ) -> AppResult<Self> {
        if capacity == 0 || worker_count == 0 {
            return Err(AppError::InvalidConfig(
                "任务队列容量和工作线程数必须大于零".into(),
            ));
        }
        let state = Arc::new(QueueState {
            heap: Mutex::new(BinaryHeap::new()),
            queued_ids: Mutex::new(HashSet::new()),
            items: Semaphore::new(0),
            slots: Arc::new(Semaphore::new(capacity)),
            sequence: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            shutdown: CancellationToken::new(),
        });
        let (results, _) = broadcast::channel(capacity.max(worker_count).max(16));
        let queue = Self {
            state,
            registry,
            supervisor,
            results,
            workers: Arc::new(Mutex::new(Vec::with_capacity(worker_count))),
        };
        let mut workers = queue.workers.lock().await;
        for _ in 0..worker_count {
            let worker_queue = queue.clone();
            workers.push(tokio::spawn(async move {
                worker_queue.worker_loop().await;
            }));
        }
        drop(workers);
        Ok(queue)
    }

    pub async fn enqueue(&self, envelope: TaskEnvelope) -> AppResult<()> {
        envelope.validate()?;
        if self.state.closed.load(AtomicOrdering::SeqCst) {
            return Err(AppError::Conflict("任务队列已关闭".into()));
        }
        let slot = self
            .state
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|error| match error {
                TryAcquireError::NoPermits => AppError::Conflict("任务队列已满".into()),
                TryAcquireError::Closed => AppError::Conflict("任务队列已关闭".into()),
            })?;
        if self.state.closed.load(AtomicOrdering::SeqCst) {
            return Err(AppError::Conflict("任务队列已关闭".into()));
        }
        if self.supervisor.contains(&envelope.local_task_id).await {
            return Err(AppError::Conflict(format!(
                "任务正在执行：{}",
                envelope.local_task_id
            )));
        }
        let mut queued_ids = self.state.queued_ids.lock().await;
        if !queued_ids.insert(envelope.local_task_id.clone()) {
            return Err(AppError::Conflict(format!(
                "任务已在队列中：{}",
                envelope.local_task_id
            )));
        }
        let sequence = self.state.sequence.fetch_add(1, AtomicOrdering::SeqCst);
        self.state.heap.lock().await.push(QueuedTask {
            sequence,
            envelope,
            _slot: slot,
        });
        drop(queued_ids);
        self.state.items.add_permits(1);
        Ok(())
    }

    pub fn subscribe_results(&self) -> broadcast::Receiver<TaskQueueResult> {
        self.results.subscribe()
    }

    pub async fn queued_count(&self) -> usize {
        self.state.heap.lock().await.len()
    }

    pub fn is_closed(&self) -> bool {
        self.state.closed.load(AtomicOrdering::SeqCst)
    }

    pub async fn cancel(&self, task_id: &str) -> AppResult<()> {
        let mut heap = self.state.heap.lock().await;
        let before = heap.len();
        heap.retain(|task| task.envelope.local_task_id != task_id);
        if heap.len() != before {
            self.state.queued_ids.lock().await.remove(task_id);
            if let Ok(permit) = self.state.items.try_acquire() {
                permit.forget();
            }
            let _ = self.results.send(TaskQueueResult {
                local_task_id: task_id.into(),
                outcome: JobOutcome::Cancelled,
            });
            return Ok(());
        }
        drop(heap);
        for _ in 0..4 {
            if self.supervisor.contains(task_id).await {
                return self.supervisor.cancel(task_id).await;
            }
            tokio::task::yield_now().await;
        }
        Err(AppError::NotFound(format!(
            "任务不在队列或监督器中：{task_id}"
        )))
    }

    pub async fn shutdown(&self, timeout: Duration) -> Vec<(String, JobOutcome)> {
        let mut shutdown_results = self.results.subscribe();
        self.state.closed.store(true, AtomicOrdering::SeqCst);
        self.state.shutdown.cancel();
        self.state.slots.close();
        self.state.items.close();
        let queued = self
            .state
            .heap
            .lock()
            .await
            .drain()
            .map(|task| task.envelope.local_task_id)
            .collect::<Vec<_>>();
        self.state.queued_ids.lock().await.clear();
        let mut outcomes = Vec::with_capacity(queued.len());
        for task_id in queued {
            let outcome = JobOutcome::Cancelled;
            let _ = self.results.send(TaskQueueResult {
                local_task_id: task_id.clone(),
                outcome: outcome.clone(),
            });
            outcomes.push((task_id, outcome));
        }
        let deadline = tokio::time::Instant::now() + timeout;
        self.supervisor.cancel_all().await;
        let supervisor_outcomes = self.supervisor.shutdown(timeout).await;
        outcomes.extend(supervisor_outcomes);
        let mut workers = self.workers.lock().await;
        for mut worker in workers.drain(..) {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if tokio::time::timeout(remaining, &mut worker).await.is_err() {
                worker.abort();
                let _ = worker.await;
            }
        }
        while let Ok(result) = shutdown_results.try_recv() {
            if !outcomes
                .iter()
                .any(|(task_id, _)| task_id == &result.local_task_id)
            {
                outcomes.push((result.local_task_id, result.outcome));
            }
        }
        outcomes.sort_by(|left, right| left.0.cmp(&right.0));
        outcomes
    }

    async fn worker_loop(&self) {
        loop {
            tokio::select! {
                _ = self.state.shutdown.cancelled() => break,
                permit = self.state.items.acquire() => {
                    let Ok(permit) = permit else {
                        break;
                    };
                    permit.forget();
                    let Some(task) = self.state.heap.lock().await.pop() else {
                        continue;
                    };
                    self.state
                        .queued_ids
                        .lock()
                        .await
                        .remove(&task.envelope.local_task_id);
                    let task_id = task.envelope.local_task_id.clone();
                    let handler = self
                        .registry
                        .get(&task.envelope.domain_type, &task.envelope.operation_type);
                    let outcome = match handler {
                        Ok(handler) => {
                            let envelope = task.envelope;
                            match self
                                .supervisor
                                .spawn(&task_id, move |cancellation| handler(envelope, cancellation))
                                .await
                            {
                                Ok(_) => match self.supervisor.wait_until_finished(&task_id).await {
                                    Ok(()) => self
                                        .supervisor
                                        .join(&task_id)
                                        .await
                                        .unwrap_or_else(|error| JobOutcome::Failed(error.to_string())),
                                    Err(_) if self.state.closed.load(AtomicOrdering::SeqCst) => {
                                        JobOutcome::Cancelled
                                    }
                                    Err(error) => JobOutcome::Failed(error.to_string()),
                                },
                                Err(error) => JobOutcome::Failed(error.to_string()),
                            }
                        }
                        Err(error) => JobOutcome::Failed(error.to_string()),
                    };
                    let _ = self.results.send(TaskQueueResult {
                        local_task_id: task_id,
                        outcome,
                    });
                }
            }
        }
    }
}
