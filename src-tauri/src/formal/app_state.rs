use std::sync::Arc;

use super::config::AppPaths;
use super::job_supervisor::JobSupervisor;
use super::local_store::LocalStore;
use super::runtime_registry::ProjectRuntimeRegistry;
use super::secret_store::SecretStore;
use crate::infrastructure::local_sqlite::task_repository::TaskRepository;
use crate::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
use crate::runtime::event_bus::TaskEventBus;
use crate::runtime::task_queue::{TaskHandlerRegistry, TaskQueue};

pub struct FormalAppState {
    pub local_store: LocalStore,
    pub secret_store: Arc<dyn SecretStore>,
    pub runtime_registry: ProjectRuntimeRegistry,
    pub job_supervisor: JobSupervisor,
    pub task_handler_registry: TaskHandlerRegistry,
    pub task_recovery_registry: crate::infrastructure::task_recovery::TaskRecoveryRegistry,
    pub task_queue: TaskQueue,
    pub task_event_bus: TaskEventBus,
    pub task_repository: TaskRepository,
    pub task_event_pipeline: TaskEventPipeline,
    pub paths: AppPaths,
}
