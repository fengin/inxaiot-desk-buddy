//! 本机临时制品清理的窄范围一体机实机回归。
//! 复用现有测试装配，正式检查、提交、队列、SSH、共享结果及本机收尾均实际执行。
//! 只允许142随机双库及79/121随机目录；独立无网络测试容器不替换现有业务服务。
use super::*;
use inxaiot_desk_buddy_lib::infrastructure::remote::RemoteSession;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Read;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const IMAGE_FILE: &str = "device-edge-web-1.0.1.Alpha.20260812.tar";
const IMAGE_TAG: &str = "192.168.3.48:5000/inxvision/device-edge-web:1.0.1.Alpha.20260812";
const ORIGINAL_CONTAINERS: [&str; 4] = [
    "inx-device-edge",
    "inx-rule-engine",
    "inx-device-edge-web",
    "inx-edge-emqx",
];

fn source_image_id(path: &Path) -> Result<String> {
    let mut archive = tar::Archive::new(std::fs::File::open(path)?);
    let mut config = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() == Path::new("manifest.json") {
            require(entry.size() <= 1024 * 1024, "镜像清单超过测试上限")?;
            let mut contents = String::new();
            entry.read_to_string(&mut contents)?;
            let manifest: Value = serde_json::from_str(&contents)?;
            require(
                manifest.as_array().is_some_and(|v| v.len() == 1),
                "测试归档必须只有一个镜像",
            )?;
            require(
                manifest[0]["RepoTags"] == json!([IMAGE_TAG]),
                "测试镜像附带其他标签，禁止覆盖",
            )?;
            config = manifest[0]["Config"].as_str().map(str::to_owned);
        }
    }
    let config = config.ok_or("测试镜像缺少配置摘要")?;
    let mut archive = tar::Archive::new(std::fs::File::open(path)?);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() == Path::new(&config) {
            require(entry.size() <= 1024 * 1024, "镜像配置超过测试上限")?;
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            return Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))));
        }
    }
    Err("测试镜像配置不存在".into())
}

fn require(ok: bool, message: &'static str) -> Result<()> {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn persist(dir: &Path, report: &Value) -> Result<()> {
    std::fs::write(dir.join("result.json"), serde_json::to_vec_pretty(report)?)?;
    Ok(())
}
async fn command(
    session: &RemoteSession,
    program: &str,
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
) -> Result<String> {
    let result = session
        .run(
            &ExecRequest {
                program: program.into(),
                args,
                stdin,
                env: BTreeMap::new(),
                total_timeout: Duration::from_secs(120),
                inactivity_timeout: Duration::from_secs(45),
            },
            &tokio_util::sync::CancellationToken::new(),
            &NoopRemoteOutputSink,
        )
        .await?;
    require(
        result.exit_status == 0,
        "隔离测试远端命令失败；保留现场供核实",
    )?;
    Ok(result.stdout)
}
async fn connect(cfg: &TestConfig, node: &TestNode) -> Result<RemoteSession> {
    Ok(RusshConnector::default()
        .connect(
            &RemoteTarget {
                host: node.ip.clone(),
                port: 22,
                connect_timeout: Duration::from_secs(10),
            },
            &RemoteAuth::PrivateKey {
                username: line(&cfg.description, "一体机ssh用户：").into(),
                private_key: SecretValue::new(read_private_key(cfg)?),
                passphrase: None,
            },
            HostKeyPolicy::Capture,
        )
        .await?)
}
async fn baseline(session: &RemoteSession) -> Result<Value> {
    let script = r#"import json,subprocess,os
names=['inx-device-edge','inx-rule-engine','inx-device-edge-web','inx-edge-emqx']
raw=json.loads(subprocess.check_output(['docker','inspect']+names))
print(json.dumps({'current':os.path.realpath('/opt/data/deploy/current'),'containers':sorted([{'name':r['Name'],'id':r['Id'],'image':r['Image'],'running':r['State']['Running'],'started':r['State']['StartedAt']}for r in raw],key=lambda r:r['name']),'imageId':subprocess.check_output(['docker','image','inspect','--format','{{.Id}}','192.168.3.48:5000/inxvision/device-edge-web:1.0.1.Alpha.20260812']).decode().strip()}))"#;
    Ok(serde_json::from_str(
        &command(session, "python3", vec!["-c".into(), script.into()], None).await?,
    )?)
}
fn compose(token: &str) -> String {
    format!(
        "name: inxcleanup{token}\nservices:\n  cleanup-probe:\n    image: ${{PROBE_IMAGE}}\n    container_name: inx-cleanup-{token}\n    entrypoint: [\"/bin/sh\", \"-c\", \"while :; do sleep 10; done\"]\n    network_mode: none\n    healthcheck:\n      test: [\"CMD\", \"/bin/sh\", \"-c\", \"true\"]\n      interval: 1s\n      timeout: 1s\n      retries: 3\n"
    )
}
async fn prepare_remote(session: &RemoteSession, remote_root: &str, token: &str) -> Result<()> {
    let script = r#"import json,os,re,subprocess,sys
x=json.load(sys.stdin);root=x['root'];token=x['token']
assert re.fullmatch(r'/opt/data/inx-artifact-cleanup-[a-f0-9]{32}',root) and root.endswith(token)
assert not os.path.lexists(root)
os.mkdir(root);open(root+'/.scope','w').write(token)
release=root+'/deploy/releases/baseline';os.makedirs(release);os.makedirs(root+'/data/config')
open(release+'/docker-compose.yml','w').write(x['compose'])
open(release+'/.env','w').write('RELEASE_VERSION=baseline\nPROBE_IMAGE='+x['image']+'\n')
json.dump({'schemaVersion':1,'version':'baseline','images':[]},open(release+'/manifest.json','w'))
json.dump({'fixture':token},open(root+'/data/config/host-info.json','w'))
os.symlink(release,root+'/deploy/current')
subprocess.check_call(['docker','compose','--env-file',release+'/.env','-f',release+'/docker-compose.yml','up','-d'],stdout=subprocess.DEVNULL)
print('prepared')"#;
    let input =
        json!({"root":remote_root,"token":token,"compose":compose(token),"image":IMAGE_TAG});
    command(
        session,
        "python3",
        vec!["-c".into(), script.into()],
        Some(serde_json::to_vec(&input)?),
    )
    .await?;
    Ok(())
}
async fn remote_facts(session: &RemoteSession, remote_root: &str, token: &str) -> Result<Value> {
    let script = r#"import json,os,re,subprocess,sys,glob
x=json.load(sys.stdin);root=x['root'];token=x['token']
assert re.fullmatch(r'/opt/data/inx-artifact-cleanup-[a-f0-9]{32}',root) and root.endswith(token)
assert open(root+'/.scope').read()==token
current=os.path.realpath(root+'/deploy/current');assert current.startswith(root+'/deploy/releases/')
container=json.loads(subprocess.check_output(['docker','inspect','inx-cleanup-'+token]))[0]
print(json.dumps({'current':current,'running':container['State']['Running'],'health':container['State'].get('Health',{}).get('Status'),'imageId':container['Image'],'releaseTarCount':len(glob.glob(current+'/images/*.tar')),'serviceTarCount':len(glob.glob(root+'/deploy/service-upgrades/*/*.tar')),'stagingTarCount':len(glob.glob(root+'/data/.inxaiot-desk-buddy/*/*/*.tar')),'metadataExists':all(os.path.isfile(current+'/'+f) for f in ['.env','docker-compose.yml','manifest.json','checksums/sha256.txt','images/cleanup-probe.tag']),'backupCount':len(glob.glob(root+'/data/backup/*'))}))"#;
    let output = command(
        session,
        "python3",
        vec!["-c".into(), script.into()],
        Some(serde_json::to_vec(
            &json!({"root":remote_root,"token":token}),
        )?),
    )
    .await?;
    Ok(serde_json::from_str(&output)?)
}
async fn cleanup_remote(session: &RemoteSession, remote_root: &str, token: &str) -> Result<()> {
    let script = r#"import json,os,re,subprocess,sys,shutil
x=json.load(sys.stdin);root=x['root'];token=x['token']
assert re.fullmatch(r'/opt/data/inx-artifact-cleanup-[a-f0-9]{32}',root) and root.endswith(token)
assert os.path.realpath(root)==root and open(root+'/.scope').read()==token
current=os.path.realpath(root+'/deploy/current');assert current.startswith(root+'/deploy/releases/')
subprocess.check_call(['docker','compose','--env-file',current+'/.env','-f',current+'/docker-compose.yml','down','--remove-orphans'],stdout=subprocess.DEVNULL)
assert not subprocess.check_output(['docker','ps','-aq','--filter','name=^inx-cleanup-'+token+'$']).strip()
shutil.rmtree(root);assert not os.path.lexists(root)
print('removed')"#;
    command(
        session,
        "python3",
        vec!["-c".into(), script.into()],
        Some(serde_json::to_vec(
            &json!({"root":remote_root,"token":token}),
        )?),
    )
    .await?;
    Ok(())
}
async fn create_scope(admin: &MySqlPool, schema: &str, token: &str) -> Result<()> {
    require(
        regex::Regex::new(r"^inxaiot_desk_buddy_clean[bw]_[a-f0-9]{32}$")?.is_match(schema),
        "随机库名不合法",
    )?;
    sqlx::query(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4"))
        .execute(admin)
        .await?;
    sqlx::query(&format!(
        "CREATE TABLE `{schema}`.__cleanup_scope(id INT PRIMARY KEY,marker VARCHAR(32) NOT NULL)"
    ))
    .execute(admin)
    .await?;
    sqlx::query(&format!(
        "INSERT INTO `{schema}`.__cleanup_scope VALUES(1,?)"
    ))
    .bind(token)
    .execute(admin)
    .await?;
    Ok(())
}
async fn drop_scope(admin: &MySqlPool, schema: &str, token: &str) -> Result<()> {
    require(
        regex::Regex::new(r"^inxaiot_desk_buddy_clean[bw]_[a-f0-9]{32}$")?.is_match(schema),
        "随机库名不合法",
    )?;
    let marker: String = sqlx::query_scalar(&format!(
        "SELECT marker FROM `{schema}`.__cleanup_scope WHERE id=1"
    ))
    .fetch_one(admin)
    .await?;
    require(marker == token, "随机库标记不匹配，未删除")?;
    sqlx::query(&format!("DROP DATABASE `{schema}`"))
        .execute(admin)
        .await?;
    Ok(())
}
fn input(cfg: &TestConfig, mode: DeploymentMode, nodes: &[TestNode]) -> DeploymentPlanInput {
    DeploymentPlanInput {
        mode,
        target_macs: nodes.iter().map(|n| n.mac_normalized.clone()).collect(),
        image_files: vec![DeploymentImageInput {
            service_name: "cleanup-probe".into(),
            file_path: cfg
                .project_root
                .join("test/images")
                .join(IMAGE_FILE)
                .to_string_lossy()
                .into_owned(),
            image_tag: IMAGE_TAG.into(),
        }],
        artifact_path: String::new(),
        artifact_name: String::new(),
        artifact_version: String::new(),
        service_name: (mode == DeploymentMode::ServiceUpgrade).then(|| "cleanup-probe".into()),
        image_name: None,
        service_image_environment_variable: None,
        images: BTreeMap::new(),
        batch_size: 2,
        concurrency: 2,
    }
}
async fn execute_mode(
    state: &FormalAppState,
    project: &str,
    cfg: &TestConfig,
    nodes: &[TestNode],
    mode: DeploymentMode,
) -> Result<Value> {
    let preflight_id = Uuid::now_v7().to_string();
    let preflight = Stage75BPreflightAdapter::tracked(state, &preflight_id)
        .preflight(project, &input(cfg, mode, nodes))
        .await?;
    require(
        preflight.ready,
        "隔离一体机检查未通过；日志保留在本机测试目录",
    )?;
    let snapshot = preflight
        .execution_snapshot
        .as_ref()
        .ok_or("未生成执行快照")?;
    require(snapshot.targets.len() == 2, "检查目标数量不符")?;
    let submission = Stage75BSubmissionAdapter::new(state)
        .submit(project, &preflight_id, snapshot)
        .await?;
    let task_dir = state.paths.project_task_dir(project, &submission.task_id)?;
    tokio::time::timeout(Duration::from_secs(1200), async {
        loop {
            let task = state.task_repository.get(&submission.task_id).await?;
            if task.state.is_terminal() || task.state == TaskState::FinalizingFailed {
                return Ok::<_, Box<dyn std::error::Error>>(task);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| "一体机任务未按时结束，不清理仍在执行的现场")??;
    let task = state.task_repository.get(&submission.task_id).await?;
    require(
        task.state == TaskState::Succeeded,
        "隔离一体机部署未成功；保留现场",
    )?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while task_dir.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| "一体机完成后本机镜像制品未清理")?;
    require(
        tokio::fs::metadata(&task.log_path).await?.len() > 0,
        "一体机清理丢失任务日志",
    )?;
    let view = Stage75BQueryAdapter::new(state)
        .task(project, &task.id)
        .await?;
    require(
        view.success_count == 2 && view.failure_count == 0,
        "一体机逐台结果不完整",
    )?;
    let operation = view.operation_id.as_deref().ok_or("未保存共享操作")?;
    let history = Stage75BQueryAdapter::new(state)
        .history_detail(project, operation)
        .await?;
    require(
        history.targets.len() == 2 && history.targets.iter().all(|t| t.state == "succeeded"),
        "清理后共享历史不可查询",
    )?;
    Ok(
        json!({"mode":mode_code(mode),"taskId":task.id,"operationId":operation,"taskArtifactsRemoved":true,"taskLogRetained":true,"historyRetained":true}),
    )
}

#[tokio::test]
#[ignore = "仅142随机双库和79/121独立目录/容器；正式工作台整包与单服升级并验证清理，需显式门禁"]
async fn isolated_full_and_service_upgrade_cleanup_on_79_and_121() -> Result<()> {
    require(
        std::env::var("INX_AIO_ARTIFACT_REAL").as_deref() == Ok("YES"),
        "缺少一体机清理实机门禁",
    )?;
    let expected_remote_tars = std::env::var("INX_AIO_ARTIFACT_EXPECT_REMOTE_TARS").ok();
    require(
        expected_remote_tars
            .as_deref()
            .is_none_or(|value| value == "removed"),
        "远端镜像清理验收门禁无效",
    )?;
    let require_remote_tar_cleanup = expected_remote_tars.is_some();
    let mut cfg = config();
    require(
        cfg.host == "192.168.3.142" && cfg.port == 3306,
        "仅允许142测试数据库",
    )?;
    let nodes = real_nodes();
    require(
        line(&cfg.description, "一体机IP:")
            .split('/')
            .map(str::trim)
            .collect::<Vec<_>>()
            == vec!["192.168.3.79", "192.168.3.121"],
        "测试节点说明已变化",
    )?;
    let token = Uuid::now_v7().simple().to_string();
    let business = format!("inxaiot_desk_buddy_cleanb_{token}");
    let shared = format!("inxaiot_desk_buddy_cleanw_{token}");
    let remote_root = format!("/opt/data/inx-artifact-cleanup-{token}");
    let directory = cfg
        .project_root
        .join(".review-tools/artifact-cleanup-20261024")
        .join(format!("aio-{token}"));
    std::fs::create_dir_all(&directory)?;
    let mut report = json!({"passed":false,"token":token,"businessSchema":business,"sharedSchema":shared,"remoteRoot":remote_root,"remotePrepared":[],"remoteRemoved":[],"baseline":{},"finalBaseline":{},"runs":[],"productionDataUsed":false,"schemasRemoved":false,"restoreRequired":false});
    report["remoteTarCleanupRequired"] = json!(require_remote_tar_cleanup);
    persist(&directory, &report)?;
    println!("一体机清理实机记录：{}", directory.display());
    let admin = admin_pool(&cfg).await;
    create_scope(&admin, &business, &token).await?;
    create_scope(&admin, &shared, &token).await?;
    for table in ["op_edge_aio_server", "t_project_building"] {
        sqlx::query(&format!(
            "CREATE TABLE `{business}`.`{table}` LIKE inxvision_iot_dev_demo.`{table}`"
        ))
        .execute(&admin)
        .await?;
    }
    for (i, node) in nodes.iter().enumerate() {
        sqlx::query(&format!("INSERT INTO `{business}`.op_edge_aio_server(id,name,ip,mac,building_id,addr_alias,status,platform_ip,platform_port) VALUES(?,?,?,?,0,'隔离清理测试',1,?,?)"))
            .bind(77001i64+i as i64).bind(&node.name).bind(&node.ip).bind(&node.mac_normalized)
            .bind(&cfg.host).bind(line(&cfg.description, "平台API：").rsplit_once(':').ok_or("测试平台API格式无效")?.1)
            .execute(&admin).await?;
    }
    cfg.platform_schema = business.clone();
    cfg.schema = shared.clone();
    let state = state_at(&directory.join("app-data")).await;
    let adapter = Stage75Adapter::new(&state);
    let project = adapter
        .create_project(project_input(&cfg, "本机镜像清理隔离验收"))
        .await?
        .project
        .id;
    let pools = DualMySqlPools::connect(&MySqlProjectConfig {
        host: cfg.host.clone(),
        port: cfg.port,
        username: cfg.username.clone(),
        password: SecretValue::new(cfg.password.clone()),
        platform_schema: business.clone(),
        workbench_schema: shared.clone(),
        connect_timeout: Duration::from_secs(10),
        tls_mode: DatabaseTlsMode::Disabled,
    })
    .await?;
    WorkbenchStore::new(pools.workbench.clone())
        .migrate()
        .await?;
    adapter.switch_project(&project).await?;
    adapter.login_project(&project, login_request(&cfg)).await?;
    let mut draft = release_draft(&cfg);
    draft.values.aio_data_root = format!("{remote_root}/data");
    draft.values.aio_deploy_root = format!("{remote_root}/deploy");
    draft.values.compose_template = compose(&token);
    draft.values.env_template =
        "RELEASE_VERSION={{ release.version }}\nPROBE_IMAGE={{ images.cleanup-probe }}\n".into();
    adapter.save_release_profile(&project, draft).await?;
    WorkbenchAioRepository::new(pools.workbench.clone())
        .apply_inventory(ApplyInventoryWrite {
            file_name: "cleanup-fixture.csv".into(),
            operator_name: "cleanup-fixture".into(),
            instance_id: token.clone(),
            classification_counts: json!({"managed":2}),
            assets: nodes
                .iter()
                .enumerate()
                .map(|(i, n)| InventoryAssetWrite {
                    mac_normalized: n.mac_normalized.clone(),
                    display_mac: n.mac_normalized.clone(),
                    name: n.name.clone(),
                    ip: n.ip.clone(),
                    building_id: None,
                    region_id: None,
                    addr_alias: Some("隔离清理测试".into()),
                    floor: None,
                    location: Some("隔离清理测试".into()),
                    remark: None,
                    platform_aio_id: Some((77001 + i).to_string()),
                    management_state: "managed".into(),
                    source: "platform".into(),
                    expected_version: None,
                })
                .collect(),
        })
        .await?;
    let mut sessions = Vec::new();
    let source_id = source_image_id(&cfg.project_root.join("test/images").join(IMAGE_FILE))?;
    report["sourceImageId"] = json!(source_id);
    for node in &nodes {
        let session = connect(&cfg, node).await?;
        let before = baseline(&session).await?;
        require(
            before["containers"].as_array().is_some_and(|v| {
                v.len() == ORIGINAL_CONTAINERS.len() && v.iter().all(|c| c["running"] == true)
            }),
            "既有一体机服务基线不正常，停止测试",
        )?;
        require(
            before["imageId"] == source_id,
            "测试源包与远端现有镜像ID不同，禁止覆盖共享标签",
        )?;
        report["baseline"][&node.ip] = before;
        let captured = adapter
            .capture_host_key(
                &project,
                HostKeyCaptureRequest {
                    host: node.ip.clone(),
                    port: Some(22),
                },
            )
            .await?;
        adapter
            .confirm_host_key(
                &project,
                ConfirmHostKeyRequest {
                    host: captured.host,
                    port: captured.port,
                    algorithm: captured.algorithm,
                    fingerprint: captured.fingerprint,
                    replace_changed: false,
                },
            )
            .await?;
        sessions.push((node.ip.clone(), session));
    }
    persist(&directory, &report)?;
    let outcome = async {
        for (host, session) in &sessions {
            report["restoreRequired"] = json!(true);
            persist(&directory, &report)?;
            prepare_remote(session, &remote_root, &token).await?;
            report["remotePrepared"]
                .as_array_mut()
                .unwrap()
                .push(json!(host));
            persist(&directory, &report)?;
        }
        let source = cfg.project_root.join("test/images").join(IMAGE_FILE);
        let source_sha = inxaiot_desk_buddy_lib::domain::aio::release::sha256_file(&source)?;
        for mode in [DeploymentMode::FullUpgrade, DeploymentMode::ServiceUpgrade] {
            let result = execute_mode(&state, &project, &cfg, &nodes, mode).await?;
            report["runs"].as_array_mut().unwrap().push(result);
            persist(&directory, &report)?;
            for (host, session) in &sessions {
                let facts = remote_facts(session, &remote_root, &token).await?;
                require(
                    facts["running"] == true
                        && facts["health"] == "healthy"
                        && facts["metadataExists"] == true
                        && facts["stagingTarCount"] == 0
                        && facts["backupCount"].as_u64().is_some_and(|count| count > 0),
                    "独立部署运行、配置或远端临时目录核实失败",
                )?;
                require(
                    facts["imageId"] == report["baseline"][host]["imageId"],
                    "测试导入后镜像身份不符",
                )?;
                if require_remote_tar_cleanup {
                    require(
                        facts["releaseTarCount"] == 0 && facts["serviceTarCount"] == 0,
                        "正式部署成功后本次远端镜像tar未清理",
                    )?;
                }
                require(
                    baseline(session).await? == report["baseline"][host],
                    "部署期间既有四个业务服务或原current发生变化",
                )?;
                report[format!("{}-facts", mode_code(mode))][host] = facts;
            }
            persist(&directory, &report)?;
        }
        require(
            inxaiot_desk_buddy_lib::domain::aio::release::sha256_file(&source)? == source_sha,
            "清理改变了原始镜像文件",
        )?;
        report["sourceImagePreserved"] = json!(true);
        report["remoteTarCleanupVerified"] = json!(require_remote_tar_cleanup);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    // 先等待本轮所有执行器停止；若仍在执行则保留目录，避免测试收尾删除活动输入。
    state.task_queue.shutdown(Duration::from_secs(30)).await;
    if outcome.is_ok() {
        for (host, session) in &sessions {
            cleanup_remote(session, &remote_root, &token).await?;
            report["remoteRemoved"]
                .as_array_mut()
                .unwrap()
                .push(json!(host));
            let after = baseline(session).await?;
            require(
                after == report["baseline"][host],
                "既有服务基线发生变化，需继续核实",
            )?;
            report["finalBaseline"][host] = after;
            persist(&directory, &report)?;
        }
        report["restoreRequired"] = json!(false);
    }
    state.runtime_registry.close_all().await;
    pools.close().await;
    state.local_store.close().await;
    if outcome.is_ok() && report["restoreRequired"] == false {
        drop_scope(&admin, &business, &token).await?;
        drop_scope(&admin, &shared, &token).await?;
        report["schemasRemoved"] = json!(true);
        report["passed"] = json!(true);
    }
    persist(&directory, &report)?;
    for (_, session) in sessions {
        session.disconnect().await?;
    }
    admin.close().await;
    outcome
}
