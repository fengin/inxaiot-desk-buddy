use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use inxaiot_desk_buddy_lib::core::error::AppError;
use inxaiot_desk_buddy_lib::runtime::job_supervisor::{JobOutcome, JobSupervisor};
use inxaiot_desk_buddy_lib::runtime::task_queue::{TaskEnvelope, TaskHandlerRegistry, TaskQueue};
use tokio::sync::{Notify, Semaphore, mpsc};

fn envelope(id: &str, priority: i32, operation_type: &str) -> TaskEnvelope {
    TaskEnvelope {
        local_task_id: id.into(),
        local_project_id: "project-a".into(),
        domain_type: "test".into(),
        operation_type: operation_type.into(),
        resource_keys: vec!["resource-a".into()],
        priority,
        payload_ref: None,
    }
}

#[tokio::test]
async fn supervisor_reports_completion_cancellation_panic_and_forced_abort() {
    let supervisor = JobSupervisor::default();
    supervisor
        .spawn("complete", |_| async { Ok(()) })
        .await
        .expect("spawn complete job");
    assert_eq!(
        supervisor.join("complete").await.expect("join complete"),
        JobOutcome::Completed
    );

    supervisor
        .spawn("cancel", |cancellation| async move {
            cancellation.cancelled().await;
            Err(AppError::Cancelled)
        })
        .await
        .expect("spawn cancellable job");
    supervisor.cancel("cancel").await.expect("cancel job");
    assert_eq!(
        supervisor.join("cancel").await.expect("join cancelled"),
        JobOutcome::Cancelled
    );

    supervisor
        .spawn("panic", |_| async move {
            panic!("isolated task panic");
            #[allow(unreachable_code)]
            Ok(())
        })
        .await
        .expect("spawn panic job");
    assert_eq!(
        supervisor.join("panic").await.expect("join panic"),
        JobOutcome::Panicked
    );

    supervisor
        .spawn("stubborn", |_| async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(())
        })
        .await
        .expect("spawn stubborn job");
    assert_eq!(
        supervisor.shutdown(Duration::from_millis(50)).await,
        vec![("stubborn".into(), JobOutcome::Aborted)]
    );
    assert_eq!(supervisor.active_count().await, 0);
}

#[tokio::test]
async fn bounded_queue_orders_waiting_tasks_by_priority_and_rejects_duplicates() {
    let registry = TaskHandlerRegistry::default();
    let first_release = Arc::new(Notify::new());
    let (started_tx, mut started_rx) = mpsc::channel::<String>(8);
    let release = first_release.clone();
    registry
        .register("test", "ordered", move |task, _| {
            let started_tx = started_tx.clone();
            let release = release.clone();
            async move {
                started_tx
                    .send(task.local_task_id.clone())
                    .await
                    .map_err(|_| AppError::Conflict("测试接收器已关闭".into()))?;
                if task.local_task_id == "first" {
                    release.notified().await;
                }
                Ok(())
            }
        })
        .expect("register ordered handler");
    let queue = TaskQueue::start(4, 1, registry, JobSupervisor::default())
        .await
        .expect("start queue");
    let mut results = queue.subscribe_results();
    queue
        .enqueue(envelope("first", 0, "ordered"))
        .await
        .expect("enqueue first");
    assert_eq!(started_rx.recv().await.as_deref(), Some("first"));
    queue
        .enqueue(envelope("low", 1, "ordered"))
        .await
        .expect("enqueue low");
    assert!(matches!(
        queue.enqueue(envelope("low", 1, "ordered")).await,
        Err(AppError::Conflict(_))
    ));
    queue
        .enqueue(envelope("high", 10, "ordered"))
        .await
        .expect("enqueue high");
    assert_eq!(queue.queued_count().await, 2);
    first_release.notify_one();
    assert_eq!(started_rx.recv().await.as_deref(), Some("high"));
    assert_eq!(started_rx.recv().await.as_deref(), Some("low"));
    let mut completed = Vec::new();
    for _ in 0..3 {
        let result = results.recv().await.expect("queue result");
        assert_eq!(result.outcome, JobOutcome::Completed);
        completed.push(result.local_task_id);
    }
    completed.sort();
    assert_eq!(completed, vec!["first", "high", "low"]);
    assert!(queue.shutdown(Duration::from_secs(1)).await.is_empty());
}

#[tokio::test]
async fn queue_cancellation_and_missing_handler_have_distinct_results() {
    let registry = TaskHandlerRegistry::default();
    let started = Arc::new(Notify::new());
    let started_for_handler = started.clone();
    registry
        .register("test", "cancellable", move |_, cancellation| {
            let started = started_for_handler.clone();
            async move {
                started.notify_one();
                cancellation.cancelled().await;
                Err(AppError::Cancelled)
            }
        })
        .expect("register cancellable handler");
    registry
        .register("test", "fails", |_, _| async move {
            Err(AppError::InvalidConfig("expected failure".into()))
        })
        .expect("register failure handler");
    let queue = TaskQueue::start(4, 1, registry, JobSupervisor::default())
        .await
        .expect("start queue");
    let mut results = queue.subscribe_results();
    queue
        .enqueue(envelope("cancel-me", 1, "cancellable"))
        .await
        .expect("enqueue cancellable");
    started.notified().await;
    queue
        .cancel("cancel-me")
        .await
        .expect("cancel running task");
    let cancelled = results.recv().await.expect("cancel result");
    assert_eq!(cancelled.local_task_id, "cancel-me");
    assert_eq!(cancelled.outcome, JobOutcome::Cancelled);

    queue
        .enqueue(envelope("fails", 1, "fails"))
        .await
        .expect("enqueue failure");
    let failed = results.recv().await.expect("failure result");
    assert!(matches!(failed.outcome, JobOutcome::Failed(_)));

    queue
        .enqueue(envelope("missing", 1, "missing"))
        .await
        .expect("enqueue missing handler");
    let missing = results.recv().await.expect("missing result");
    assert!(matches!(missing.outcome, JobOutcome::Failed(_)));
    assert!(queue.shutdown(Duration::from_secs(1)).await.is_empty());
}

#[tokio::test]
async fn queued_task_can_be_cancelled_before_handler_dispatch() {
    let registry = TaskHandlerRegistry::default();
    let release = Arc::new(Semaphore::new(0));
    let (started_tx, mut started_rx) = mpsc::channel::<String>(4);
    let handler_release = release.clone();
    registry
        .register("test", "queued-cancel", move |task, _| {
            let started_tx = started_tx.clone();
            let release = handler_release.clone();
            async move {
                started_tx
                    .send(task.local_task_id.clone())
                    .await
                    .map_err(|_| AppError::Conflict("测试接收器已关闭".into()))?;
                if task.local_task_id == "blocker" {
                    release
                        .acquire()
                        .await
                        .map_err(|_| AppError::Conflict("测试信号量已关闭".into()))?
                        .forget();
                }
                Ok(())
            }
        })
        .expect("register queued cancel handler");
    let queue = TaskQueue::start(4, 1, registry, JobSupervisor::default())
        .await
        .expect("start queue");
    let mut results = queue.subscribe_results();
    queue
        .enqueue(envelope("blocker", 1, "queued-cancel"))
        .await
        .expect("enqueue blocker");
    assert_eq!(started_rx.recv().await.as_deref(), Some("blocker"));
    queue
        .enqueue(envelope("queued", 1, "queued-cancel"))
        .await
        .expect("enqueue queued task");
    queue.cancel("queued").await.expect("cancel queued task");
    let cancelled = results.recv().await.expect("queued cancellation result");
    assert_eq!(cancelled.local_task_id, "queued");
    assert_eq!(cancelled.outcome, JobOutcome::Cancelled);
    release.add_permits(1);
    assert_eq!(
        results.recv().await.expect("blocker result").outcome,
        JobOutcome::Completed
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), started_rx.recv())
            .await
            .is_err()
    );
    assert!(queue.shutdown(Duration::from_secs(1)).await.is_empty());
}

#[tokio::test]
async fn screen_and_gateway_handlers_share_global_concurrency_without_aio_dependencies() {
    let registry = TaskHandlerRegistry::default();
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Semaphore::new(0));
    let (started_tx, mut started_rx) = mpsc::channel::<String>(8);
    for domain in ["screen", "gateway"] {
        let active = active.clone();
        let peak = peak.clone();
        let release = release.clone();
        let started_tx = started_tx.clone();
        registry
            .register(domain, "test_handler", move |task, _| {
                let active = active.clone();
                let peak = peak.clone();
                let release = release.clone();
                let started_tx = started_tx.clone();
                async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    started_tx
                        .send(task.domain_type)
                        .await
                        .map_err(|_| AppError::Conflict("测试接收器已关闭".into()))?;
                    release
                        .acquire()
                        .await
                        .map_err(|_| AppError::Conflict("测试信号量已关闭".into()))?
                        .forget();
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                }
            })
            .expect("register cross-domain handler");
    }
    assert_eq!(
        registry.registered_keys().expect("keys"),
        vec![
            ("gateway".into(), "test_handler".into()),
            ("screen".into(), "test_handler".into())
        ]
    );
    let queue = TaskQueue::start(4, 2, registry, JobSupervisor::default())
        .await
        .expect("queue");
    let mut results = queue.subscribe_results();
    let mut screen = envelope("screen-1", 1, "test_handler");
    screen.domain_type = "screen".into();
    let mut gateway = envelope("gateway-1", 1, "test_handler");
    gateway.domain_type = "gateway".into();
    let mut waiting = envelope("screen-2", 1, "test_handler");
    waiting.domain_type = "screen".into();
    queue.enqueue(screen).await.expect("screen");
    queue.enqueue(gateway).await.expect("gateway");
    queue.enqueue(waiting).await.expect("waiting");
    let mut domains = vec![
        started_rx.recv().await.expect("first domain"),
        started_rx.recv().await.expect("second domain"),
    ];
    domains.sort();
    assert_eq!(domains, vec!["gateway", "screen"]);
    assert_eq!(peak.load(Ordering::SeqCst), 2);
    assert_eq!(queue.queued_count().await, 1);
    release.add_permits(3);
    for _ in 0..3 {
        assert_eq!(
            results.recv().await.expect("result").outcome,
            JobOutcome::Completed
        );
    }
    assert!(queue.shutdown(Duration::from_secs(1)).await.is_empty());
}

#[tokio::test]
async fn queue_capacity_rejects_immediately_and_shutdown_reports_waiting_task() {
    let registry = TaskHandlerRegistry::default();
    let started = Arc::new(Notify::new());
    let started_for_handler = started.clone();
    registry
        .register("screen", "blocking", move |_, cancellation| {
            let started = started_for_handler.clone();
            async move {
                started.notify_one();
                cancellation.cancelled().await;
                Err(AppError::Cancelled)
            }
        })
        .expect("handler");
    let queue = TaskQueue::start(2, 1, registry, JobSupervisor::default())
        .await
        .expect("queue");
    let mut running = envelope("running", 1, "blocking");
    running.domain_type = "screen".into();
    queue.enqueue(running).await.expect("running");
    started.notified().await;
    let mut waiting = envelope("waiting", 1, "blocking");
    waiting.domain_type = "screen".into();
    queue.enqueue(waiting).await.expect("waiting");
    let mut overflow = envelope("overflow", 1, "blocking");
    overflow.domain_type = "screen".into();
    let error = queue
        .enqueue(overflow)
        .await
        .expect_err("capacity must reject");
    assert!(error.to_string().contains("队列已满"));
    let outcomes = queue.shutdown(Duration::from_secs(1)).await;
    assert!(outcomes.contains(&("waiting".into(), JobOutcome::Cancelled)));
    assert!(outcomes.contains(&("running".into(), JobOutcome::Cancelled)));
    assert!(queue.is_closed());
}
