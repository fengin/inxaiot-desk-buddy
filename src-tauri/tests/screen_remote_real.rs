//! 单屏查看的只读实机回归。截图只在内存中校验，不写图片、凭据或网页内容。
//! 默认忽略，仅允许测试说明列出的42/82及其硬件序列号；不唤醒、输入、登录或改配置。
#[path = "common/screen_test_support.rs"]
mod support;

use base64::{Engine, engine::general_purpose::STANDARD};
use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::smart_screen::model::ScreenFields,
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{
            device::{AdbDevice, AndroidTools},
            remote_screen::{self, RemoteScreenSessions},
        },
        stage75_adapter::Stage75Adapter,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path, time::Instant};
use tokio_util::sync::CancellationToken;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const OWNER: &str = "remote-screen-readonly-real";
const TARGETS: [(&str, &str, &str); 2] = [
    ("192.168.3.42", "10", "9c000c3c3643071205d"),
    ("192.168.3.82", "4", "A47A5CK00524"),
];

fn selected_targets(
    value: Option<&str>,
) -> TestResult<Vec<(&'static str, &'static str, &'static str)>> {
    match value {
        None => Ok(TARGETS.to_vec()),
        Some(ip) => TARGETS
            .iter()
            .copied()
            .find(|target| target.0 == ip)
            .map(|target| vec![target])
            .ok_or_else(|| "截图实机目标只允许192.168.3.42或192.168.3.82".into()),
    }
}

#[test]
fn remote_capture_target_selector_rejects_unlisted_addresses() {
    assert_eq!(selected_targets(None).unwrap().len(), 2);
    assert_eq!(
        selected_targets(Some("192.168.3.42")).unwrap(),
        vec![TARGETS[0]]
    );
    assert_eq!(
        selected_targets(Some("192.168.3.82")).unwrap(),
        vec![TARGETS[1]]
    );
    for value in [
        "",
        "192.168.3.74",
        "192.168.3.42:5555",
        "192.168.1.1",
        " 192.168.3.82",
    ] {
        assert!(selected_targets(Some(value)).is_err());
    }
}

fn require(value: bool, message: &'static str) -> TestResult<()> {
    if value { Ok(()) } else { Err(message.into()) }
}

fn digest(value: &Value) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(value).unwrap()))
}

fn persist(directory: &Path, report: &Value) -> TestResult<()> {
    std::fs::write(
        directory.join("result.json"),
        serde_json::to_vec_pretty(report)?,
    )?;
    Ok(())
}

async fn provider(device: &AdbDevice, ip: &str, domain: &str, method: &str) -> TestResult<Value> {
    require(
        matches!(
            (domain, method),
            ("config", "readConfig" | "readKiosk" | "getCapabilities") | ("home", "readHome")
        ),
        "只允许读取配置和系统策略的方法",
    )?;
    let uri = format!("content://chat.xiaoxin.app.workbench.{domain}");
    let raw = device
        .shell(
            ip,
            &["content", "call", "--uri", &uri, "--method", method],
            CancellationToken::new(),
        )
        .await?;
    let expression = regex::Regex::new(r"result=([A-Za-z0-9+/=]+)")?;
    let encoded = expression.captures(&raw).ok_or("未读到管理接口结果")?;
    let value: Value = serde_json::from_slice(&STANDARD.decode(&encoded[1])?)?;
    require(value["status"] == "ok", "只读配置或系统策略未确认成功")?;
    Ok(value["data"].clone())
}

async fn baseline(device: &AdbDevice, ip: &str, serial: &str) -> TestResult<Value> {
    let actual = device
        .shell(ip, &["getprop", "ro.serialno"], CancellationToken::new())
        .await?;
    require(actual == serial, "硬件序列号与固定测试目标不一致")?;
    let capabilities = provider(device, ip, "config", "getCapabilities").await?;
    let config = provider(device, ip, "config", "readConfig").await?;
    let h5 = config["environments"]["test"]["h5Url"]
        .as_str()
        .ok_or("未读到测试环境H5地址")?;
    require(
        reqwest::Url::parse(h5)?.host_str() == Some("192.168.3.44"),
        "测试屏H5地址不是待保留的44地址",
    )?;
    let kiosk = provider(device, ip, "config", "readKiosk").await?;
    require(
        kiosk["storageConfirmed"] == true && kiosk["deviceOwner"] == true,
        "测试屏锁定及OWNER状态未确认",
    )?;
    let keys = [
        "configured",
        "enabled",
        "deviceOwner",
        "activityReady",
        "lockTaskMode",
        "active",
        "storageConfirmed",
    ];
    let kiosk_state = keys
        .into_iter()
        .map(|key| (key.into(), kiosk[key].clone()))
        .collect::<serde_json::Map<String, Value>>();
    let home = provider(device, ip, "home", "readHome").await?;
    let timeout = device
        .shell(
            ip,
            &["settings", "get", "system", "screen_off_timeout"],
            CancellationToken::new(),
        )
        .await?;
    Ok(
        json!({"serialSha256":hex::encode(Sha256::digest(serial.as_bytes())),"configSha256":digest(&config),
        "appVersionName":capabilities["appVersionName"],"appVersionCode":capabilities["appVersionCode"],
        "h5Host":"192.168.3.44","kiosk":kiosk_state,"home":home,"screenTimeout":timeout}),
    )
}

async fn row_counts(state: &FormalAppState) -> TestResult<Value> {
    let mut values = serde_json::Map::new();
    for table in [
        "local_task",
        "local_task_target",
        "local_screen_task_data",
        "local_screen_observation",
        "local_screen_write_intent",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(state.local_store.pool())
            .await?;
        values.insert(table.into(), json!(count));
    }
    Ok(Value::Object(values))
}

fn non_database_files(path: &Path) -> TestResult<Vec<String>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(path)?;
        if matches!(
            relative.to_str(),
            Some("local.db" | "local.db-wal" | "local.db-shm")
        ) {
            continue;
        }
        files.push(relative.to_string_lossy().into_owned());
    }
    files.sort();
    Ok(files)
}

#[tokio::test]
#[ignore = "只读连接说明中的42/82测试屏，校验截图内存传输及现有配置不变；需显式门禁"]
async fn screen_remote_capture_on_four_and_ten_inch_preserves_configuration() -> TestResult<()> {
    require(
        std::env::var("INX_REMOTE_SCREEN_REAL").as_deref() == Ok("YES"),
        "缺少截图实机验证门禁",
    )?;
    let selection = match std::env::var("INX_REMOTE_SCREEN_TARGET") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err("截图实机目标配置无效".into()),
    };
    let targets = selected_targets(selection.as_deref())?;
    let configured = support::live_screens()?;
    for &(ip, size, _) in &targets {
        require(
            configured
                .iter()
                .any(|screen| screen.ip == ip && screen.size == size),
            "固定测试屏不在当前测试说明中",
        )?;
    }
    let directory = support::evidence_dir("remote-screen-readonly")?;
    let state = support::state_at(&directory.join("app-data"), false).await;
    let sessions = RemoteScreenSessions::default();
    let mut report = json!({"passed":false,"mode":"readonly-capture","targets":[],"screenshotsWritten":false,
        "selectedTargets":targets.iter().map(|target|target.0).collect::<Vec<_>>(),"bothSizesCovered":false,
        "deviceWriteCommandsIssued":false,"productionDataUsed":false,"configurationUnchanged":false});
    persist(&directory, &report)?;
    println!(
        "截图实机验收元数据：{}",
        directory.join("result.json").display()
    );
    println!(
        "本次仅验证已选择目标：{}",
        targets
            .iter()
            .map(|target| target.0)
            .collect::<Vec<_>>()
            .join("、")
    );
    let result = async {
        let project = Stage75Adapter::new(&state).create_project(support::local_input()).await?.project.id;
        let repository = ScreenRepository::new(state.local_store.pool().clone());
        let device = AdbDevice::new(AndroidTools::discover()?);
        for &(ip, size, serial) in &targets {
            device.connect(ip, CancellationToken::new()).await?;
            let before = baseline(&device, ip, serial).await?;
            let screen = repository.save_local(&project, &ScreenFields {
                name:format!("{size}寸只读截图验收"),ip:ip.into(),size:size.into(),..Default::default()
            }, None, None).await?;
            let counts = row_counts(&state).await?;
            let files = non_database_files(&state.paths.data_dir)?;
            let session = remote_screen::open(&state, &sessions, OWNER, &project, &screen).await?;
            require(session.ip == ip && session.screen_id == screen, "截图会话目标不一致")?;
            let mut frames = Vec::new();
            let mut frame_ids = BTreeSet::new();
            let mut dimensions = None;
            for _ in 0..3 {
                let started = Instant::now();
                let frame = remote_screen::capture(&state, &sessions, OWNER, &project, &session.session_id).await?;
                let bytes = STANDARD.decode(&frame.png_base64)?;
                require(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "实机截图不是完整PNG")?;
                require(frame.session_id == session.session_id && frame.width > 0 && frame.height > 0, "截图绑定或尺寸无效")?;
                require(frame_ids.insert(frame.frame_id.clone()), "重复使用了旧截图标识")?;
                if let Some(value) = dimensions { require(value == (frame.width, frame.height), "只读截图期间屏幕尺寸发生变化")?; }
                dimensions = Some((frame.width, frame.height));
                frames.push(json!({"width":frame.width,"height":frame.height,"bytes":bytes.len(),
                    "sha256":hex::encode(Sha256::digest(&bytes)),"frameId":frame.frame_id,"capturedAt":frame.captured_at,
                    "backendElapsedMs":frame.elapsed_ms,"totalElapsedMs":started.elapsed().as_millis()}));
            }
            remote_screen::close(&sessions, OWNER, &project, &session.session_id)?;
            require(remote_screen::capture(&state, &sessions, OWNER, &project, &session.session_id).await.is_err(), "关闭后的截图会话仍可使用")?;
            let after = baseline(&device, ip, serial).await?;
            require(before == after, "截图前后配置或系统策略不一致，请保留现场核实")?;
            require(row_counts(&state).await? == counts, "截图产生了运维任务或设备记录")?;
            require(non_database_files(&state.paths.data_dir)? == files, "截图在工作台数据目录生成了文件")?;
            report["targets"].as_array_mut().unwrap().push(json!({"ip":ip,"size":size,"baseline":before,
                "frames":frames,"configurationUnchanged":true,"noTaskOrImageFilesCreated":true,"closedSessionRejected":true}));
            persist(&directory, &report)?;
            println!("{size}寸测试屏：3帧内存截图、配置保持及关闭会话验证通过");
        }
        report["configurationUnchanged"] = json!(true);
        Ok::<(), Box<dyn std::error::Error>>(())
    }.await;
    sessions.close_owner(OWNER);
    support::close(state).await;
    report["passed"] = json!(result.is_ok());
    report["bothSizesCovered"] = json!(result.is_ok() && targets.len() == TARGETS.len());
    persist(&directory, &report)?;
    result
}

#[tokio::test]
#[ignore = "只初始化固定.review-tools桌面验收目录，不连接网络或设备；需显式门禁且原目录无数据库"]
async fn seed_isolated_remote_screen_desktop_data() -> TestResult<()> {
    require(
        std::env::var("INX_REMOTE_SCREEN_DESKTOP_SEED").as_deref() == Ok("YES"),
        "缺少隔离桌面数据初始化门禁",
    )?;
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("无法确定工作台工程目录")?
        .canonicalize()?;
    let review_root = repository.join(".review-tools");
    let directory = review_root.join("remote-screen-20261026");
    let data = directory.join("desktop-data");
    // 路径为本测试固定值；不读取外部数据目录选择器，也不复用任何现存数据库。
    for path in [&review_root, &directory, &data] {
        if path.exists() {
            require(!path.is_symlink(), "桌面验收目录不能是符号链接")?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                require(
                    std::fs::symlink_metadata(path)?.file_attributes() & 0x400 == 0,
                    "桌面验收目录不能是重解析点",
                )?;
            }
            require(
                path.canonicalize()?.starts_with(&repository),
                "桌面验收目录超出工程范围",
            )?;
        }
    }
    for file in ["local.db", "local.db-wal", "local.db-shm"] {
        require(
            !data.join(file).exists(),
            "桌面验收已有数据库，禁止覆盖或混用",
        )?;
    }
    if data.exists() {
        require(
            std::fs::read_dir(&data)?.next().is_none(),
            "桌面验收数据目录必须为空",
        )?;
    }
    require(
        !directory.join("desktop-seed.json").exists(),
        "桌面验收种子记录已存在，禁止覆盖",
    )?;
    let state = support::state_at(&data, false).await;
    let outcome = async {
        let adapter = Stage75Adapter::new(&state);
        let mut input = support::local_input();
        input.name = "远程屏幕隔离验收（本机测试）".into();
        let project = adapter.create_project(input).await?.project.id;
        adapter.switch_project(&project).await?;
        let screens = ScreenRepository::new(state.local_store.pool().clone());
        let mut ids = Vec::new();
        for (ip, size, name) in [
            ("192.168.3.42", "10", "10寸屏42 · 隔离验收"),
            ("192.168.3.74", "4", "4寸屏74 · 隔离验收"),
            ("192.168.3.82", "4", "4寸屏82 · 隔离验收"),
        ] {
            let id = screens
                .save_local(
                    &project,
                    &ScreenFields {
                        name: name.into(),
                        ip: ip.into(),
                        size: size.into(),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await?;
            ids.push(json!({"screenId":id,"name":name,"ip":ip,"size":size}));
        }
        let tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_task")
            .fetch_one(state.local_store.pool())
            .await?;
        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_project_session")
            .fetch_one(state.local_store.pool())
            .await?;
        require(
            tasks == 0 && sessions == 0,
            "隔离种子不应产生任务或平台会话",
        )?;
        Ok::<Value, Box<dyn std::error::Error>>(json!({"passed":true,"dataDirectory":data,
            "projectId":project,"projectName":"远程屏幕隔离验收（本机测试）","screens":ids,
            "networkAccessed":false,"deviceCommandsIssued":false,"productionDataUsed":false,
            "productionDatabaseCopied":false,"tasksCreated":0,"credentialsIncluded":false}))
    }
    .await;
    support::close(state).await;
    let report = outcome?;
    std::fs::create_dir_all(&directory)?;
    std::fs::write(
        directory.join("desktop-seed.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!(
        "隔离桌面数据已初始化：{}",
        directory.join("desktop-seed.json").display()
    );
    Ok(())
}
