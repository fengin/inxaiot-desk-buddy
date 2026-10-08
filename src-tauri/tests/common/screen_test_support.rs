use inxaiot_desk_buddy_lib::{
    domain::common::project::ProjectInput,
    formal::{
        app_state::FormalAppState, config::AppPaths, local_store::LocalStore,
        runtime_registry::ProjectRuntimeRegistry, secret_store::MemorySecretStore,
    },
    infrastructure::{
        local_sqlite::task_repository::TaskRepository,
        logging::{redactor::SensitiveValueRedactor, task_event_pipeline::TaskEventPipeline},
        smart_screen::tasks,
    },
    runtime::{
        event_bus::TaskEventBus,
        job_supervisor::JobSupervisor,
        task_queue::{TaskHandlerRegistry, TaskQueue},
    },
};
use std::sync::Arc;
use std::time::Duration;

// 实机目标只取本机测试说明；不保留历史 IP，也不靠局域网扫描猜测目标。
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct LiveScreen {
    pub name: String,
    pub size: String,
    pub ip: String,
}

#[allow(dead_code)]
pub fn live_screens() -> Result<Vec<LiveScreen>, Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let description = std::fs::read_to_string(root.join("test/测试数据说明.txt"))?;
    let mut result = Vec::new();
    for line in description.lines() {
        let Some((name, address)) = line.trim().split_once('：') else {
            continue;
        };
        let size = if name.starts_with("4寸屏") {
            "4"
        } else if name.starts_with("10寸屏") {
            "10"
        } else {
            continue;
        };
        let address: std::net::SocketAddr = address.trim().parse()?;
        if address.port() != 5555 || !address.is_ipv4() {
            return Err(format!("{name} 必须明确使用 IPv4:5555").into());
        }
        if result
            .iter()
            .any(|item: &LiveScreen| item.ip == address.ip().to_string())
        {
            return Err("测试说明包含重复屏地址".into());
        }
        result.push(LiveScreen {
            name: name.into(),
            size: size.into(),
            ip: address.ip().to_string(),
        });
    }
    if result.is_empty() {
        return Err("测试说明中没有智能屏地址".into());
    }
    Ok(result)
}

#[allow(dead_code)]
pub fn live_screen(size: &str) -> Result<LiveScreen, Box<dyn std::error::Error>> {
    live_screens()?
        .into_iter()
        .find(|item| item.size == size)
        .ok_or_else(|| format!("测试说明中没有 {size} 寸屏").into())
}

// 安装包由操作者明确指定。版本和架构由正式 APK 解析逻辑读取，禁止默认回退旧包。
#[allow(dead_code)]
pub fn live_apk_path() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let path = std::env::var_os("INX_SCREEN_TEST_APK")
        .ok_or("缺少 INX_SCREEN_TEST_APK，必须明确指定待验证 APK")?;
    let path = std::path::PathBuf::from(path).canonicalize()?;
    if path.extension().and_then(|value| value.to_str()) != Some("apk") {
        return Err("INX_SCREEN_TEST_APK 必须指向 APK 文件".into());
    }
    Ok(path)
}

#[allow(dead_code)]
pub fn evidence_dir(case: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let root = std::env::var_os("INX_SCREEN_TEST_EVIDENCE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join(".review-tools/screen-live")
        });
    let directory = root.join(format!("{case}-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&directory)?;
    Ok(directory)
}

pub async fn state_at(path: &std::path::Path, devices: bool) -> Arc<FormalAppState> {
    let paths = AppPaths::from_data_dir(path).unwrap();
    paths.ensure().unwrap();
    let local_store = LocalStore::open(&paths.local_db).await.unwrap();
    let task_repository = TaskRepository::new(local_store.pool().clone());
    let task_event_bus = TaskEventBus::new(64).unwrap();
    let pipeline = TaskEventPipeline::new(
        task_repository.clone(),
        task_event_bus.clone(),
        SensitiveValueRedactor::default(),
    );
    let job_supervisor = JobSupervisor::default();
    let registry = TaskHandlerRegistry::default();
    let queue = TaskQueue::start(16, 2, registry.clone(), job_supervisor.clone())
        .await
        .unwrap();
    let state = Arc::new(FormalAppState {
        paths,
        local_store,
        task_repository,
        task_event_bus,
        task_event_pipeline: pipeline,
        secret_store: Arc::new(MemorySecretStore::default()),
        runtime_registry: ProjectRuntimeRegistry::default(),
        job_supervisor,
        task_handler_registry: registry.clone(),
        task_queue: queue,
        task_recovery_registry:
            inxaiot_desk_buddy_lib::infrastructure::task_handlers::built_in_recovery_registry(),
    });
    if devices {
        let mut events = state.task_queue.subscribe_results();
        let event_repository = state.task_repository.clone();
        let event_paths = state.paths.clone();
        tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                inxaiot_desk_buddy_lib::infrastructure::task_runtime::reconcile_queue_result(
                    &event_repository,
                    &event_paths,
                    &event,
                )
                .await;
            }
        });
        for action in ["register", "merge"] {
            let weak = Arc::downgrade(&state);
            registry
                .register("smart_screen", action, move |envelope, cancel| {
                    let weak = weak.clone();
                    async move {
                        let state = weak.upgrade().expect("test state alive");
                        inxaiot_desk_buddy_lib::infrastructure::smart_screen::registration::run(
                            &state, envelope, cancel,
                        )
                        .await
                    }
                })
                .unwrap();
        }
        for action in ["version_sync", "status"] {
            let weak = Arc::downgrade(&state);
            registry
                .register("smart_screen", action, move |envelope, cancel| {
                    let weak = weak.clone();
                    async move {
                        let state = weak.upgrade().expect("test state alive");
                        inxaiot_desk_buddy_lib::infrastructure::smart_screen::value_updates::run(
                            &state, envelope, cancel,
                        )
                        .await
                    }
                })
                .unwrap();
        }
        for action in inxaiot_desk_buddy_lib::domain::smart_screen::operation::WRITE_ACTIONS {
            let weak = Arc::downgrade(&state);
            registry
                .register("smart_screen", action, move |envelope, cancel| {
                    let weak = weak.clone();
                    async move {
                        let state = weak.upgrade().expect("test state alive");
                        inxaiot_desk_buddy_lib::infrastructure::smart_screen::maintenance::run(
                            &state, envelope, cancel,
                        )
                        .await
                    }
                })
                .unwrap();
        }
        for action in inxaiot_desk_buddy_lib::domain::smart_screen::operation::READ_ACTIONS {
            let weak = Arc::downgrade(&state);
            registry
                .register("smart_screen", action, move |envelope, cancel| {
                    let weak = weak.clone();
                    async move {
                        let state = weak.upgrade().expect("test state alive");
                        tasks::run(&state, envelope, cancel).await
                    }
                })
                .unwrap();
        }
    }
    state
}
#[allow(dead_code)]
pub fn local_input() -> ProjectInput {
    ProjectInput {
        name: "智能屏本机测试".into(),
        platform_url: String::new(),
        db_host: String::new(),
        db_port: 3306,
        db_user: String::new(),
        db_tls_enabled: false,
        db_password: None,
        business_db: String::new(),
        workbench_db: "inxaiot_desk_buddy".into(),
    }
}
pub async fn close(state: Arc<FormalAppState>) {
    state.task_queue.shutdown(Duration::from_secs(5)).await;
    state.runtime_registry.close_all().await;
    state.local_store.close().await;
}
