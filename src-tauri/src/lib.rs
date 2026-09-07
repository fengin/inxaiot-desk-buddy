pub mod application;
pub mod core;
pub mod domain;
pub mod formal;
pub mod infrastructure;
pub mod interface;
pub mod runtime;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{Emitter, Manager};

use crate::formal::app_state::FormalAppState;
use crate::formal::config::AppPaths;
use crate::formal::job_supervisor::JobSupervisor;
use crate::formal::local_store::LocalStore;
use crate::formal::logging::init_file_logging;
use crate::formal::project_repository::LocalProjectRepository;
use crate::formal::runtime_registry::ProjectRuntimeRegistry;
use crate::formal::secret_store::OsSecretStore;
use crate::infrastructure::agent_asset::verify_embedded_agent;
use crate::infrastructure::data_directory::DataDirectoryManager;
use crate::infrastructure::local_sqlite::task_repository::TaskRepository;
use crate::infrastructure::logging::redactor::SensitiveValueRedactor;
use crate::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
use crate::infrastructure::process_lock::DataDirectoryProcessLock;
use crate::infrastructure::task_data_lifecycle::TaskDataLifecycle;
use crate::infrastructure::task_handlers::register_production_task_handlers;
use crate::infrastructure::task_runtime::{
    prepare_shutdown_tasks, reconcile_queue_result, reconcile_shutdown_outcomes,
    reconcile_untracked_active_tasks, retry_pending_local_finalizations,
};
use crate::interface::commands::aio_assets::{
    apply_inventory_import, discard_inventory_import, get_edge_node_detail,
    get_latest_inventory_import, list_edge_nodes, preview_inventory_import,
    update_inventory_import_selection,
};
use crate::interface::commands::application_lifecycle::{
    ApplicationExitImpact, ExitConfirmationState, confirm_application_exit,
};
use crate::interface::commands::data_directory::{
    get_data_directory_status, schedule_data_directory_rollback, schedule_data_directory_switch,
};
use crate::interface::commands::deployment_workflow::{
    get_deployment_task, get_operation_history_detail, list_operation_history,
    preflight_deployment, submit_deployment,
};
use crate::interface::commands::diagnostics::get_system_diagnostics;
use crate::interface::commands::project_database::{
    get_workbench_schema_status, initialize_or_upgrade_workbench_schema,
};
use crate::interface::commands::project_management::{
    capture_host_key, check_project_session, confirm_host_key, create_local_project,
    create_project_login_challenge, delete_local_project, get_project_session, get_release_profile,
    list_host_keys, list_local_projects, login_project, logout_project, open_release_agent_script,
    replace_release_agent_script, save_release_profile, switch_project, test_project_connection,
    update_local_project, validate_release_profile,
};
use crate::interface::commands::release_artifacts::inspect_service_image;
use crate::interface::commands::service_inspection::check_edge_node_services;
use crate::interface::commands::task_activity::{
    cancel_local_task, clear_finished_local_tasks, clear_task_logs, list_local_tasks,
    list_task_logs, retry_local_task_finalization,
};
use crate::runtime::event_bus::TaskEventBus;
use crate::runtime::task_queue::{TaskHandlerRegistry, TaskQueue};

const TASK_QUEUE_CAPACITY: usize = 100;
const TASK_QUEUE_WORKERS: usize = 4;
const APPLICATION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());
    #[cfg(feature = "desktop-e2e")]
    let builder = builder.plugin(tauri_plugin_wdio_webdriver::init());
    let application = builder
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let confirmation = window.state::<ExitConfirmationState>();
                if confirmation.take_confirmation() {
                    return;
                }
                let state = window.state::<FormalAppState>();
                let counts = tauri::async_runtime::block_on(state.task_repository.active_counts())
                    .unwrap_or_default();
                if counts.total > 0 {
                    api.prevent_close();
                    let _ = window.emit(
                        "application-exit-impact",
                        ApplicationExitImpact {
                            active_task_count: counts.total,
                            queued_task_count: counts.queued,
                            running_task_count: counts.running,
                            wait_timeout_seconds: APPLICATION_SHUTDOWN_TIMEOUT.as_secs() as u32,
                        },
                    );
                }
            }
        })
        .setup(|app| {
            let (data_directory, data_directory_manager) =
                DataDirectoryManager::resolve(app.path().app_data_dir()?)?;
            let paths = AppPaths::from_data_dir(data_directory)?;
            paths.ensure()?;
            let data_directory_process_lock =
                DataDirectoryProcessLock::acquire(&paths.process_lock)?;
            verify_embedded_agent()?;
            let logging_guard = init_file_logging(&paths.logs_dir)?;
            tracing::info!(
                instance_id = crate::infrastructure::client_instance::application_instance_id(),
                "workbench client instance initialized"
            );
            let task_event_bus = TaskEventBus::new(512)?;
            let setup_app_handle = app.handle().clone();
            let state = tauri::async_runtime::block_on(async {
                let local_store = LocalStore::open(&paths.local_db).await?;
                let task_repository = TaskRepository::new(local_store.pool().clone());
                task_repository.recover_interrupted().await?;
                let task_data_lifecycle = TaskDataLifecycle::new(&paths);
                match task_repository.list_artifact_cleanup_candidates().await {
                    Ok(tasks) => {
                        for task in tasks {
                            if let Err(error) = task_data_lifecycle.finalize_task(
                                &task.local_project_id,
                                &task.id,
                                task.state,
                            ) {
                                tracing::warn!(
                                    task_id = %task.id,
                                    error = %crate::core::log_safety::safe_error(&error),
                                    "startup task artifact cleanup deferred"
                                );
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(error = %crate::core::log_safety::safe_error(&error), "startup task cleanup scan deferred");
                    }
                }
                if let Err(error) = task_data_lifecycle.sweep_expired_logs() {
                    tracing::warn!(error = %crate::core::log_safety::safe_error(&error), "startup task log retention sweep deferred");
                }
                let job_supervisor = JobSupervisor::default();
                let task_handler_registry = TaskHandlerRegistry::default();
                register_production_task_handlers(&task_handler_registry, setup_app_handle)?;
                let task_queue = TaskQueue::start(
                    TASK_QUEUE_CAPACITY,
                    TASK_QUEUE_WORKERS,
                    task_handler_registry.clone(),
                    job_supervisor.clone(),
                )
                .await?;
                let task_event_pipeline = TaskEventPipeline::new(
                    task_repository.clone(),
                    task_event_bus.clone(),
                    SensitiveValueRedactor::production(),
                );
                let state = FormalAppState {
                    local_store,
                    secret_store: Arc::new(OsSecretStore::new("inxaiot-desk-buddy")?),
                    runtime_registry: ProjectRuntimeRegistry::default(),
                    job_supervisor,
                    task_handler_registry,
                    task_queue,
                    task_event_bus: task_event_bus.clone(),
                    task_repository,
                    task_event_pipeline,
                    paths,
                };
                match LocalProjectRepository::new(
                    state.local_store.pool().clone(),
                    state.secret_store.clone(),
                )
                .retry_pending_secret_cleanup()
                .await
                {
                    Ok(report) if report.pending > 0 => tracing::warn!(
                        attempted = report.attempted,
                        deleted = report.deleted,
                        pending = report.pending,
                        "pending local secret cleanup remains"
                    ),
                    Ok(_) => {}
                    Err(error) => tracing::warn!(
                        error = %crate::core::log_safety::safe_error(&error),
                        "startup local secret cleanup deferred"
                    ),
                }
                retry_pending_local_finalizations(&state).await;
                Ok::<_, Box<dyn std::error::Error>>(state)
            })?;
            let mut events = state.task_event_bus.subscribe();
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) => {
                            let _ = app_handle.emit("task-event", event);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            let mut queue_results = state.task_queue.subscribe_results();
            let outcome_repository = state.task_repository.clone();
            let outcome_paths = state.paths.clone();
            let outcome_queue = state.task_queue.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match queue_results.recv().await {
                        Ok(result) => {
                            reconcile_queue_result(&outcome_repository, &outcome_paths, &result)
                                .await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            reconcile_untracked_active_tasks(
                                &outcome_repository,
                                &outcome_paths,
                                &outcome_queue,
                            )
                            .await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            app.manage(state);
            app.manage(data_directory_manager);
            app.manage(data_directory_process_lock);
            app.manage(ExitConfirmationState::default());
            app.manage(Mutex::new(logging_guard));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_local_projects,
            create_local_project,
            update_local_project,
            delete_local_project,
            test_project_connection,
            switch_project,
            create_project_login_challenge,
            login_project,
            get_project_session,
            check_project_session,
            logout_project,
            get_release_profile,
            validate_release_profile,
            save_release_profile,
            replace_release_agent_script,
            open_release_agent_script,
            list_host_keys,
            capture_host_key,
            confirm_host_key,
            list_edge_nodes,
            get_edge_node_detail,
            check_edge_node_services,
            preview_inventory_import,
            get_latest_inventory_import,
            update_inventory_import_selection,
            apply_inventory_import,
            discard_inventory_import,
            get_workbench_schema_status,
            initialize_or_upgrade_workbench_schema,
            inspect_service_image,
            preflight_deployment,
            submit_deployment,
            get_deployment_task,
            list_operation_history,
            get_operation_history_detail,
            list_local_tasks,
            list_task_logs,
            cancel_local_task,
            retry_local_task_finalization,
            clear_finished_local_tasks,
            clear_task_logs,
            confirm_application_exit,
            get_data_directory_status,
            schedule_data_directory_switch,
            schedule_data_directory_rollback,
            get_system_diagnostics
        ])
        .build(tauri::generate_context!());
    let application = match application {
        Ok(application) => application,
        Err(error) => {
            show_startup_error(&format!("工作台启动失败：{error}"));
            return;
        }
    };
    application.run(|app_handle, event| match event {
        tauri::RunEvent::ExitRequested { api, .. } => {
            let confirmation = app_handle.state::<ExitConfirmationState>();
            if confirmation.take_confirmation() {
                return;
            }
            let state = app_handle.state::<FormalAppState>();
            let counts = tauri::async_runtime::block_on(state.task_repository.active_counts())
                .unwrap_or_default();
            if counts.total > 0 {
                api.prevent_exit();
                let _ = app_handle.emit(
                    "application-exit-impact",
                    ApplicationExitImpact {
                        active_task_count: counts.total,
                        queued_task_count: counts.queued,
                        running_task_count: counts.running,
                        wait_timeout_seconds: APPLICATION_SHUTDOWN_TIMEOUT.as_secs() as u32,
                    },
                );
            }
        }
        tauri::RunEvent::Exit => {
            let state = app_handle.state::<FormalAppState>();
            tauri::async_runtime::block_on(async {
                prepare_shutdown_tasks(&state).await;
                let outcomes = state
                    .task_queue
                    .shutdown(APPLICATION_SHUTDOWN_TIMEOUT)
                    .await;
                reconcile_shutdown_outcomes(&state, &outcomes).await;
                state.runtime_registry.close_all().await;
                state.local_store.close().await;
            });
        }
        _ => {}
    });
}

#[cfg(windows)]
fn show_startup_error(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

    let title = "INX 实施工作台"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let message = message
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_startup_error(message: &str) {
    eprintln!("{message}");
}
