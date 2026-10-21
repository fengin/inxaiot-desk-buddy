#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::{project_management::ProjectManagementPort, smart_screen::ScreenAssetsPort},
    domain::{
        common::{project::ProjectInput, project_space::space_path},
        smart_screen::{model::{ScreenAsset, ScreenFields, SpaceNode}, rules::{validate_fields, validate_space}},
    },
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::assets_service::ScreenAssetsService,
        stage75_adapter::Stage75Adapter,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, io::Write, path::{Path, PathBuf}};

type SeedResult<T> = Result<T, Box<dyn std::error::Error>>;
const APP_DIRECTORY: &str = r"C:\Users\fengin\AppData\Roaming\com.inxaiot.desk-buddy.voice-gui-20261021";
const CODEX_REDIRECTED_ROAMING: &str = r"C:\Users\fengin\AppData\Local\Packages\OpenAI.Codex_2p2nqsd0c76g0\LocalCache\Roaming";
const RUN: &str = "voice-config-gui-20261021";
const MARKER: &str = ".voice-config-gui-20261021-seed.json";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureManifest {
    nonce: String,
    business_database: String,
    shared_database: String,
    state: String,
    host: String,
    markers_matched: bool,
    business_project_id: String,
    data_source_id: String,
    spaces: Vec<SpaceNode>,
}

fn require(condition: bool, message: &str) -> SeedResult<()> {
    if condition { Ok(()) } else { Err(message.to_string().into()) }
}

fn root() -> SeedResult<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR")).parent().ok_or("缺少工作台工程目录")?.to_path_buf())
}

fn fixture() -> SeedResult<FixtureManifest> {
    let path = root()?.join(".review-tools/voice-config-gui-20261021/fixture-manifest.json");
    let manifest: FixtureManifest = serde_json::from_slice(&std::fs::read(path)?)?;
    require(manifest.nonce.len() == 32 && manifest.nonce.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "GUI隔离库必须使用32位小写十六进制nonce")?;
    require(manifest.business_database == format!("inxaiot_desk_buddy_ui_b_{}", manifest.nonce)
        && manifest.shared_database == format!("inxaiot_desk_buddy_ui_w_{}", manifest.nonce),
        "GUI业务库与工作台库必须是本轮受限前缀下配对的隔离库")?;
    require(manifest.state == "prepared_verified" && manifest.markers_matched,
        "GUI隔离双库尚未完成准备与标记核验，未创建本机种子")?;
    require(manifest.business_project_id == "777001" && manifest.data_source_id.ends_with(&format!(":{}", manifest.business_database)),
        "GUI空间缓存必须使用已核验的临时业务项目与数据源")?;
    require(manifest.data_source_id.split(':').count() == 2
        && manifest.data_source_id.split(':').next().is_some_and(|value| uuid::Uuid::parse_str(value).is_ok()),
        "GUI临时数据源标识无效")?;
    let ids: BTreeSet<_> = manifest.spaces.iter().map(|space| space.id.as_str()).collect();
    require(manifest.spaces.len() == 3 && ids == BTreeSet::from(["1000", "1001", "1002"])
        && manifest.spaces.iter().all(|space| space_path(&manifest.spaces, &space.id).is_some()),
        "GUI临时空间缓存必须与已核验的三个空间节点一致")?;
    Ok(manifest)
}

fn connection_without_password(manifest: &FixtureManifest) -> SeedResult<ProjectInput> {
    // 测试说明只在内存解析。仅取地址和账号名，不取密码、登录报文或原secret_ref。
    let text = std::fs::read_to_string(root()?.join("test/测试数据说明.txt"))?;
    let value = |label: &str| text.lines().find_map(|line| line.trim().trim_start_matches('\u{feff}').strip_prefix(label))
        .map(str::trim).filter(|value| !value.is_empty()).ok_or("测试说明缺少GUI项目必需字段");
    let api = value("平台API：")?;
    let platform_url = if api.starts_with("http://") || api.starts_with("https://") { api.into() } else { format!("http://{api}") };
    let account = value("平台数据库账号密码：")?.split_once('/').map(|(username, _)| username.trim())
        .filter(|username| !username.is_empty()).ok_or("测试说明缺少数据库账号名")?;
    let input = ProjectInput {
        name: "GUI智能屏实机回归".into(), platform_url, db_host: value("平台主机：")?.into(),
        db_port: std::env::var("INX_TEST_MYSQL_PORT").ok().map(|port| port.parse()).transpose()?.unwrap_or(3306),
        db_user: account.into(), db_tls_enabled: false, db_password: None,
        business_db: manifest.business_database.clone(), workbench_db: manifest.shared_database.clone(),
    };
    input.validate_for_update()?;
    require(input.db_host == manifest.host, "当前测试说明主机与已核验的GUI隔离双库主机不一致")?;
    Ok(input)
}

fn performance_snapshot() -> SeedResult<(Vec<ScreenAsset>, Vec<SpaceNode>)> {
    let path = root()?.join(".review-tools/screen-list-performance-20261020/backend-snapshot.anonymized.json");
    let snapshot: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    // 仅导入匿名资产和空间，不导入原项目编号、任务、观测、草稿、登录态或凭据。
    let mut screens: Vec<ScreenAsset> = serde_json::from_value(snapshot["screens"].clone())?;
    let spaces: Vec<SpaceNode> = serde_json::from_value(snapshot["spaces"].clone())?;
    require(screens.len() == 269 && spaces.len() == 530, "匿名性能夹具规模不符合本轮269屏/530空间")?;
    require(spaces.iter().filter(|space| space.kind == "building").count() == 4
        && spaces.iter().filter(|space| space.kind == "floor").count() == 75
        && spaces.iter().all(|space| space.id.starts_with("sample-") && space_path(&spaces, &space.id).is_some()),
        "匿名性能空间目录不完整或包含非样本编号")?;
    let mut ids = BTreeSet::new();
    for screen in &mut screens {
        let ip = screen.fields.ip.parse::<std::net::Ipv4Addr>()?.octets();
        require(screen.source == "platform" && screen.id.starts_with("sample-") && ids.insert(screen.id.clone())
            && ip[0] == 198 && matches!(ip[1], 18 | 19), "性能屏必须为匿名样本及保留的基准测试网段，不能混入实机")?;
        validate_fields(&screen.fields, false)?;
        validate_space(&screen.fields, &spaces, false)?;
        screen.aliases.clear();
        screen.revision = 0;
    }
    Ok((screens, spaces))
}

fn approved_actual_directory(actual: &Path) -> SeedResult<bool> {
    let requested = Path::new(APP_DIRECTORY);
    let name = requested.file_name().ok_or("指定GUI目录无效")?;
    let direct = requested.parent().ok_or("指定GUI目录没有父目录")?.canonicalize()?.join(name);
    if actual == direct { return Ok(true); }
    // Codex MSIX 容器只对新AppData子目录做实际重定向。只认本机已核实的精确容器目录及同一identifier。
    let redirected = Path::new(CODEX_REDIRECTED_ROAMING);
    Ok(redirected.is_dir() && actual == redirected.canonicalize()?.join(name))
}

fn create_owned_directory() -> SeedResult<PathBuf> {
    require(cfg!(windows), "GUI种子只允许在本轮指定Windows电脑准备")?;
    let target = PathBuf::from(APP_DIRECTORY);
    if target.exists() {
        require(target.is_dir() && approved_actual_directory(&target.canonicalize()?)?, "指定GUI目录实际位置不符，拒绝访问")?;
        let marker = target.join(MARKER);
        require(marker.is_file(), "GUI目录已经存在但无本轮marker，拒绝覆盖或读取其数据库")?;
        let marker: Value = serde_json::from_slice(&std::fs::read(marker)?)?;
        require(marker["run"] == RUN && marker["appDataDirectory"] == APP_DIRECTORY,
            "GUI目录marker不属于本轮，拒绝覆盖")?;
        return Err("本轮GUI种子目录已存在；为保护已进行的界面操作，不自动重建或覆盖".into());
    }
    // 不采用环境变量或日常工作台目录回退，不复制任何bootstrap文件。
    std::fs::create_dir(&target)?;
    if !approved_actual_directory(&target.canonicalize()?)? {
        // 只撤销本次刚创建的空目录；非递归，不删除任何已有内容。
        let _ = std::fs::remove_dir(&target);
        return Err("新GUI目录实际位置不符，停止准备".into());
    }
    let mut marker = std::fs::OpenOptions::new().write(true).create_new(true).open(target.join(MARKER))?;
    marker.write_all(&serde_json::to_vec_pretty(&json!({"run":RUN,"phase":"preparing","appDataDirectory":APP_DIRECTORY}))?)?;
    marker.sync_all()?;
    Ok(target)
}

#[tokio::test]
#[ignore = "仅准备固定独立AppData的真实GUI数据，不执行任何GUI步骤、设备命令或平台写入"]
async fn seed_real_gui_projects_without_credentials() -> SeedResult<()> {
    // 所有输入先验证；验证失败时不创建AppData，更不打开正常工作台目录。
    let manifest = fixture()?;
    let platform = connection_without_password(&manifest)?;
    let (screens, spaces) = performance_snapshot()?;
    let evidence = root()?.join(".review-tools/voice-config-gui-20261021");
    require(evidence.is_dir() && !evidence.join("seed-manifest.json").exists(),
        "GUI种子安全清单已存在或本轮证据目录缺失，拒绝重复准备")?;
    let targets = support::live_screens()?;
    let pairs: BTreeSet<_> = targets.iter().map(|target| (target.ip.as_str(), target.size.as_str())).collect();
    require(targets.len() == 3 && pairs == BTreeSet::from([("192.168.3.106", "10"), ("192.168.3.74", "4"), ("192.168.3.82", "4")]),
        "GUI实机项目仅允许本轮明确授权的三台测试屏")?;
    let directory = create_owned_directory()?;
    let state = support::state_at(&directory, false).await;
    let result: SeedResult<Value> = async {
        let adapter = Stage75Adapter::new(&state);
        let repo = ScreenRepository::new(state.local_store.pool().clone());
        // 正式新建平台项目要求密码。先合法创建本机项目，再用已有编辑流程补连接资料；密码始终None。
        let mut local = support::local_input(); local.name = "GUI智能屏实机回归".into();
        local.workbench_db = manifest.shared_database.clone();
        local.validate_for_create()?;
        let project_a = adapter.create_project(local).await?.project.id;
        adapter.update_project(&project_a, platform).await?;
        let revision = repo.project_connection_revision(&project_a).await?;
        repo.replace_platform_snapshot(&project_a, &manifest.business_project_id, &manifest.data_source_id,
            &revision, &[], &manifest.spaces).await?;
        for target in targets {
            repo.save_local(&project_a, &ScreenFields {
                name: format!("GUI-{}寸-{}", target.size, target.ip.rsplit('.').next().ok_or("测试屏地址无效")?),
                ip: target.ip, mac: String::new(), size: target.size,
                space_id: Some("1001".into()), location: "GUI回归测试".into(),
            }, None, None).await?;
        }
        let mut local = support::local_input(); local.name = "GUI离线性能回归".into();
        local.validate_for_create()?;
        let project_b = adapter.create_project(local).await?.project.id;
        let business_b = "gui-offline-performance-20261021";
        repo.set_scope(&project_b, business_b, "gui-offline:anonymous-269-530").await?;
        repo.replace_platform_cache(&project_b, business_b, &screens, &spaces).await?;
        let snapshot_a = repo.snapshot(&project_a).await?;
        let snapshot_b = ScreenAssetsService::new(&state).snapshot(&project_b, true).await?;
        require(snapshot_a.screens.len() == 3 && snapshot_a.spaces.len() == 3
            && snapshot_a.screens.iter().all(|screen| screen.source == "local" && screen.fields.mac.is_empty()
                && screen.fields.space_id.as_deref() == Some("1001")), "实机项目种子数量或身份字段不符")?;
        require(snapshot_b.local_only && !snapshot_b.platform_available && snapshot_b.screens.len() == 269
            && snapshot_b.spaces.len() == 530 && snapshot_b.spaces_available
            && snapshot_b.tasks.is_empty() && snapshot_b.observations.is_empty() && snapshot_b.platform_drafts.is_empty(),
            "离线性能项目种子或平台隔离状态不符")?;
        let secrets: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_project WHERE COALESCE(db_password_secret_ref,'')<>''")
            .fetch_one(state.local_store.pool()).await?;
        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_project_session").fetch_one(state.local_store.pool()).await?;
        require(secrets == 0 && sessions == 0, "新种子不能携带数据库凭据引用或登录会话")?;
        Ok(json!({"appDataDirectory":APP_DIRECTORY,"projects":{
            "A":{"id":project_a,"screenCount":snapshot_a.screens.len(),"spaceCount":snapshot_a.spaces.len()},
            "B":{"id":project_b,"screenCount":snapshot_b.screens.len(),"spaceCount":snapshot_b.spaces.len()}
        }}))
    }.await;
    support::close(state).await;
    let output = result?;
    require(evidence.is_dir(), "本轮GUI证据目录不存在，未改用其他输出位置")?;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(evidence.join("seed-manifest.json"))?;
    file.write_all(&serde_json::to_vec_pretty(&output)?)?;
    file.sync_all()?;
    std::fs::write(directory.join(MARKER), serde_json::to_vec_pretty(&json!({
        "run":RUN,"phase":"prepared","appDataDirectory":APP_DIRECTORY
    }))?)?;
    println!("GUI_SEED_READY {}", serde_json::to_string(&output)?);
    Ok(())
}
