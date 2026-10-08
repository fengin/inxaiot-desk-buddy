#[path = "common/project_test_config.rs"]
mod project_test_config;

use inxaiot_desk_buddy_lib::{
    domain::smart_screen::model::{ResultState, ScreenTargetResult},
    formal::{
        config::AppPaths,
        local_store::LocalStore,
        resource_lease_repository::{LeaseGrant, LeaseRequest, ResourceLeaseRepository},
        workbench_store::WorkbenchStore,
    },
    infrastructure::{
        client_instance::application_instance_id,
        local_sqlite::task_repository::{CreateTask, TaskRepository},
        process_lock::DataDirectoryProcessLock,
        smart_screen::{
            platform_write,
            shared_results::{ScreenSharedResults, SharedScreenOperation},
            write_context::ScreenWriteContext,
        },
    },
};
use serde_json::{Value, json};
use sqlx::{ConnectOptions, MySqlPool};
use std::{
    collections::BTreeMap,
    error::Error,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
fn options() -> sqlx::mysql::MySqlConnectOptions {
    let cfg = project_test_config::database();
    sqlx::mysql::MySqlConnectOptions::new()
        .host(&cfg.host)
        .port(cfg.port)
        .username(&cfg.username)
        .password(&cfg.password)
        .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled)
        .disable_statement_logging()
}
async fn pool(schema: &str) -> TestResult<MySqlPool> {
    Ok(sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(3)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options().database(schema))
        .await?)
}
fn millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
fn schema_suffix(business: &str, shared: &str) -> TestResult<()> {
    let suffix = business
        .strip_prefix("inxaiot_desk_buddy_s7b_")
        .ok_or("非法测试库")?;
    if suffix.len() != 32
        || !suffix.bytes().all(|c| c.is_ascii_hexdigit())
        || shared != format!("inxaiot_desk_buddy_s7w_{suffix}")
    {
        return Err("测试库不配对".into());
    }
    Ok(())
}

struct Worker {
    child: Child,
    input: ChildStdin,
    replies: std::sync::mpsc::Receiver<Value>,
}
impl Worker {
    fn spawn(root: &Path, role: &str, business: &str, shared: &str) -> TestResult<Self> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["--exact", "step7_worker", "--ignored", "--nocapture"])
            .env("INX_STEP7_WORKER_ROOT", root)
            .env("INX_STEP7_WORKER_ROLE", role)
            .env("INX_STEP7_BUSINESS", business)
            .env("INX_STEP7_SHARED", shared)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn()?;
        let input = child.stdin.take().ok_or("缺少测试输入")?;
        let output = child.stdout.take().ok_or("缺少测试输出")?;
        let (sender, replies) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if let Some(payload) = line.strip_prefix("STEP7_REPLY ") {
                    if let Ok(value) = serde_json::from_str(payload) {
                        if sender.send(value).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Self {
            child,
            input,
            replies,
        })
    }
    fn send(&mut self, value: Value) -> TestResult {
        writeln!(self.input, "{value}")?;
        self.input.flush()?;
        Ok(())
    }
    fn receive(&self) -> TestResult<Value> {
        Ok(self.replies.recv_timeout(Duration::from_secs(30))?)
    }
    fn call(&mut self, value: Value) -> TestResult<Value> {
        self.send(value)?;
        self.receive()
    }
    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}
fn reply(value: Value) {
    println!("STEP7_REPLY {value}");
    let _ = std::io::stdout().flush();
}

#[tokio::test]
#[ignore = "仅由步骤7父测试启动；独立进程、SQLite和真实测试库，第二个来源标识仅在测试内构造"]
async fn step7_worker() -> TestResult {
    let root = PathBuf::from(std::env::var("INX_STEP7_WORKER_ROOT")?);
    let role = std::env::var("INX_STEP7_WORKER_ROLE")?;
    if role != "A" && role != "B" {
        return Err("非法测试角色".into());
    }
    let business = std::env::var("INX_STEP7_BUSINESS")?;
    let shared = std::env::var("INX_STEP7_SHARED")?;
    schema_suffix(&business, &shared)?;
    let owner = if role == "A" {
        application_instance_id().to_owned()
    } else {
        "步骤7测试电脑B-020000000002-192.0.2.2".into()
    };
    let paths = AppPaths::from_data_dir(&root)?;
    paths.ensure()?;
    let _lock = DataDirectoryProcessLock::acquire(&paths.process_lock)?;
    let local = LocalStore::open(&paths.local_db).await?;
    sqlx::query("INSERT OR IGNORE INTO local_project(id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) VALUES('step7',?,'http://test.invalid','test.invalid',3306,'test',?,?,'test-only','1','1')").bind(format!("步骤7进程{role}")).bind(&business).bind(&shared).execute(local.pool()).await?;
    let tasks = TaskRepository::new(local.pool().clone());
    let shared_pool = pool(&shared).await?;
    let write = pool(&business).await?;
    let context = ScreenWriteContext {
        business: "777001".into(),
        source: "step7-test".into(),
        read: write.clone(),
        write,
        shared: shared_pool.clone(),
        shared_schema: shared,
        operator: "step7-test".into(),
    };
    let leases = ResourceLeaseRepository::new(shared_pool.clone());
    let results = ScreenSharedResults::new(shared_pool.clone());
    let grant_file = root.join("worker-grants.json");
    let mut grants: BTreeMap<String, LeaseGrant> = if grant_file.exists() {
        serde_json::from_slice(&std::fs::read(&grant_file)?)?
    } else {
        BTreeMap::new()
    };
    reply(
        json!({"ready":true,"pid":std::process::id(),"owner":owner,"localDb":paths.local_db,"restoredGrants":grants.len()}),
    );
    let (sender, mut input) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    while let Some(line) = input.recv().await {
        let request: Value = serde_json::from_str(&line)?;
        let action = request["action"].as_str().ok_or("缺少动作")?;
        if action == "exit" {
            reply(json!({"ok":true}));
            break;
        }
        let response: TestResult<Value> = async {
            let key = request["key"].as_str().unwrap_or("7001");
            let resource = format!("777001:{key}");
            let operation = request["operation"].as_str().unwrap_or("");
            if action == "acquire" {
                let at = request["at"].as_u64().unwrap_or(0);
                if at > millis() { tokio::time::sleep(Duration::from_millis(at - millis())).await; }
                let operation = uuid::Uuid::now_v7().to_string();
                tasks.create(CreateTask { id: operation.clone(), local_project_id:"step7".into(), remote_operation_record_id:Some(operation.clone()), domain_type:"smart_screen".into(), operation_type:"version_sync".into(), name:format!("进程{role}操作"), priority:0, batch_size:1, concurrency:1, payload_ref:None, log_path:root.join(format!("{operation}.jsonl")).to_string_lossy().into_owned(), targets:vec![("smart_screen".into(),key.into())] }).await?;
                results.start(&SharedScreenOperation { id:&operation,business_project_id:"777001",action:"version_sync",name:"步骤7多进程验证",operator:"step7-test",instance_id:&owner,targets:&[resource.clone()],started_at:None }).await?;
                let request = LeaseRequest { resource_type:"smart_screen".into(), resource_key:resource, domain_type:"smart_screen".into(), operation_id:operation.clone(), owner_instance_id:owner.clone(), owner_user:"step7-test".into(), ttl:Duration::from_secs(request["ttl"].as_u64().unwrap_or(30)) };
                match leases.acquire_many(vec![request]).await {
                    Ok(mut acquired) => { let grant=acquired.remove(0); grants.insert(operation.clone(),grant.clone()); std::fs::write(&grant_file,serde_json::to_vec(&grants)?)?; Ok(json!({"acquired":true,"operation":operation,"grant":grant})) },
                    Err(error) => Ok(json!({"acquired":false,"operation":operation,"message":error.to_string()}))
                }
            } else if action == "history" {
                let rows:Vec<(String,String,String)>=sqlx::query_as("SELECT id,instance_id,state FROM operation_record WHERE domain_type='smart_screen' ORDER BY id").fetch_all(&shared_pool).await?;
                let local_ids:Vec<String>=sqlx::query_scalar("SELECT id FROM local_task ORDER BY id").fetch_all(local.pool()).await?;
                Ok(json!({"shared":rows,"local":local_ids}))
            } else {
                let grant = grants.get(operation).ok_or("缺少进程持有的占用记录")?;
                match action {
                    "valid" => Ok(json!({"valid":leases.validate_fencing(grant).await?})),
                    "heartbeat" => { leases.heartbeat(grant,Duration::from_secs(30)).await?; Ok(json!({"ok":true})) },
                    "release" => { leases.release(grant).await?; Ok(json!({"ok":true})) },
                    "recover" => { leases.recover_same_operation(vec![LeaseRequest { resource_type:grant.resource_type.clone(),resource_key:grant.resource_key.clone(),domain_type:"smart_screen".into(),operation_id:grant.operation_id.clone(),owner_instance_id:owner.clone(),owner_user:"step7-test".into(),ttl:Duration::from_secs(30) }]).await?; Ok(json!({"ok":true})) },
                    "write" => {
                        let id=grant.resource_key.strip_prefix("777001:").ok_or("非法屏范围")?;
                        let current=platform_write::record(&context.read,id).await?.ok_or("测试屏不存在")?;
                        match platform_write::set_business_value(&context,grant,id,&current.asset.fields,"app_version",current.asset.app_version.as_deref(),request["version"].as_str().ok_or("缺少版本")?).await {
                            Ok(receipt)=>Ok(json!({"written":receipt.wrote,"version":receipt.after.app_version})),
                            Err(error)=>Ok(json!({"rejected":true,"message":error.message()}))
                        }
                    },
                    "finish" => {
                        let result=ScreenTargetResult { format_version:1,screen_id:grant.resource_key.clone(),device:ResultState::NotRequired,business:ResultState::Succeeded,shared:ResultState::Pending,message:format!("进程{role}已确认结果"),..Default::default() };
                        results.save_target(operation,"777001",&result,"succeeded").await?;
                        results.finish(operation,"777001").await?;
                        leases.release(grant).await?;
                        Ok(json!({"ok":true}))
                    },
                    _ => Err("不支持的测试动作".into())
                }
            }
        }.await;
        reply(match response {
            Ok(value) => value,
            Err(error) => json!({"error":error.to_string()}),
        });
    }
    context.write.close().await;
    shared_pool.close().await;
    local.close().await;
    Ok(())
}

macro_rules! verify {
    ($condition:expr, $message:expr) => {
        if !$condition {
            return Err($message.into());
        }
    };
}

#[tokio::test]
#[ignore = "142隔离库上的双进程操作锁、过期接管及旧写入拒绝；不操作实际设备、不修改原业务库"]
async fn two_processes_enforce_locks_recovery_and_shared_history() -> TestResult {
    let admin = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(3)
        .connect_with(options())
        .await?;
    let suffix = uuid::Uuid::now_v7().simple().to_string();
    let business = format!("inxaiot_desk_buddy_s7b_{suffix}");
    let shared = format!("inxaiot_desk_buddy_s7w_{suffix}");
    let local = tempfile::tempdir()?;
    for schema in [&business, &shared] {
        sqlx::query(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4"))
            .execute(&admin)
            .await?;
    }
    let result:TestResult<Value>=async {
        sqlx::query(&format!("CREATE TABLE `{business}`.smart_terminal_screen LIKE inxvision_iot_dev_demo.smart_terminal_screen")).execute(&admin).await?;
        sqlx::query(&format!("CREATE TABLE `{business}`.t_project_building LIKE inxvision_iot_dev_demo.t_project_building")).execute(&admin).await?;
        sqlx::query(&format!("INSERT INTO `{business}`.t_project_building(id,project_info_id,parent_id,area_name,area_level) VALUES(1001,777001,0,'步骤7测试空间',3)")).execute(&admin).await?;
        sqlx::query(&format!("INSERT INTO `{business}`.smart_terminal_screen(id,name,ip,mac,size,building_id,install_address,app_version,version,status,delete_flag) VALUES(7001,'步骤7屏A','192.0.2.11','02:00:00:00:00:11','4-inch',1001,'测试','before','keep-h5',0,0),(7002,'步骤7屏B','192.0.2.12','02:00:00:00:00:12','10-inch',1001,'测试','before','keep-h5',0,0)")).execute(&admin).await?;
        let shared_pool=pool(&shared).await?; WorkbenchStore::new(shared_pool.clone()).migrate().await?;
        let mut a=Worker::spawn(&local.path().join("A"),"A",&business,&shared)?;
        let mut b=Worker::spawn(&local.path().join("B"),"B",&business,&shared)?;
        let ready_a=a.receive()?;let ready_b=b.receive()?;
        verify!(ready_a["ready"]==true&&ready_b["ready"]==true,"两个进程未启动");
        verify!(ready_a["pid"]!=ready_b["pid"]&&ready_a["owner"]!=ready_b["owner"]&&ready_a["localDb"]!=ready_b["localDb"],"进程、身份或本地目录未隔离");
        let mut checks=Vec::new();
        let at=millis()+500;
        a.send(json!({"action":"acquire","key":"7001","at":at}))?;
        b.send(json!({"action":"acquire","key":"7001","at":at}))?;
        let race_a=a.receive()?;let race_b=b.receive()?;
        verify!(race_a["acquired"].as_bool()!=race_b["acquired"].as_bool(),"同时争用未产生唯一赢家");
        let (winner,loser,won,lost)=if race_a["acquired"]==true {(&mut a,&mut b,&race_a,&race_b)} else {(&mut b,&mut a,&race_b,&race_a)};
        verify!(lost["acquired"]==false&&lost["message"].as_str().is_some_and(|s|s.contains("资源被占用")),"争用失败没有明确占用提示");
        verify!(winner.call(json!({"action":"write","operation":won["operation"],"version":"2.0.9"}))?["written"]==true,"赢家未能写入");
        verify!(winner.call(json!({"action":"finish","operation":won["operation"]}))?["ok"]==true,"赢家未释放占用");
        let retry=loser.call(json!({"action":"acquire","key":"7001"}))?;
        verify!(retry["acquired"]==true,"正常完成后第二进程不能继续操作");
        verify!(loser.call(json!({"action":"finish","operation":retry["operation"]}))?["ok"]==true,"第二进程结果未完成");
        checks.push("同屏同时提交仅一方成功，另一方明确占用；释放后可再次操作");
        let independent_a=a.call(json!({"action":"acquire","key":"7001"}))?;
        let independent_b=b.call(json!({"action":"acquire","key":"7002"}))?;
        verify!(independent_a["acquired"]==true&&independent_b["acquired"]==true,"不同屏互相阻塞");
        verify!(a.call(json!({"action":"finish","operation":independent_a["operation"]}))?["ok"]==true,"A结果失败");
        verify!(b.call(json!({"action":"finish","operation":independent_b["operation"]}))?["ok"]==true,"B结果失败");
        checks.push("不同屏允许同时持有操作权");
        let old=a.call(json!({"action":"acquire","key":"7001","ttl":3}))?;
        verify!(old["acquired"]==true,"中断前未取得占用");
        a.stop();
        verify!(b.call(json!({"action":"acquire","key":"7001"}))?["acquired"]==false,"旧占用未到期就被抢占");
        tokio::time::sleep(Duration::from_secs(4)).await;
        let takeover=b.call(json!({"action":"acquire","key":"7001"}))?;
        verify!(takeover["acquired"]==true,"占用过期后无法接管");
        verify!(takeover["grant"]["fencingToken"].as_u64()>old["grant"]["fencingToken"].as_u64(),"接管后未更换写入凭据");
        verify!(b.call(json!({"action":"write","operation":takeover["operation"],"version":"step7-new"}))?["written"]==true,"接管者写入失败");
        a=Worker::spawn(&local.path().join("A"),"A",&business,&shared)?;
        let restored=a.receive()?;
        verify!(restored["ready"]==true&&restored["restoredGrants"].as_u64().unwrap_or(0)>0,"旧进程未恢复本地记录");
        verify!(a.call(json!({"action":"valid","operation":old["operation"]}))?["valid"]==false,"旧占用仍然有效");
        for action in ["heartbeat","recover","release"] { verify!(a.call(json!({"action":action,"operation":old["operation"]}))?.get("error").is_some(),"旧进程续占、恢复或释放未被阻止"); }
        verify!(a.call(json!({"action":"write","operation":old["operation"],"version":"stale-value"}))?["rejected"]==true,"旧进程覆盖了新版本");
        verify!(b.call(json!({"action":"valid","operation":takeover["operation"]}))?["valid"]==true,"旧进程影响了新占用");
        verify!(b.call(json!({"action":"finish","operation":takeover["operation"]}))?["ok"]==true,"接管结果未保存");
        checks.push("终止A后等待真实占用过期，B接管；重启A并恢复旧凭据，续占、恢复、释放及业务写入全部被拒绝");
        let history_a=a.call(json!({"action":"history"}))?;let history_b=b.call(json!({"action":"history"}))?;
        verify!(history_a["shared"]==history_b["shared"],"共享结果不一致");
        let local_a=history_a["local"].as_array().ok_or("A本地任务缺失")?;
        let local_b=history_b["local"].as_array().ok_or("B本地任务缺失")?;
        verify!(!local_a.is_empty()&&!local_b.is_empty()&&local_a.iter().all(|id|!local_b.contains(id)),"本地任务串用");
        let row:(String,String)=sqlx::query_as(&format!("SELECT app_version,version FROM `{business}`.smart_terminal_screen WHERE id=7001")).fetch_one(&admin).await?;
        verify!(row==("step7-new".into(),"keep-h5".into()),"旧写入覆盖结果或修改H5字段");
        checks.push("两进程共享结果一致，本地任务互不串用，业务库保留接管后的结果和原H5版本");
        let output=json!({"passed":true,"aPid":ready_a["pid"],"bPid":ready_b["pid"],"restoredAPid":restored["pid"],"realOwner":ready_a["owner"],"testOwner":ready_b["owner"],"sharedOperationCount":history_a["shared"].as_array().map(Vec::len),"localTaskCounts":[local_a.len(),local_b.len()],"checks":checks});
        let _=a.call(json!({"action":"exit"}));let _=b.call(json!({"action":"exit"}));
        a.child.wait()?;b.child.wait()?; shared_pool.close().await;
        Ok(output)
    }.await;
    schema_suffix(&business, &shared)?;
    for schema in [&shared, &business] {
        sqlx::query(&format!("DROP DATABASE `{schema}`"))
            .execute(&admin)
            .await?;
    }
    admin.close().await;
    let evidence = result?;
    if let Some(path) = std::env::var_os("INX_SCREEN_STEP7_EVIDENCE") {
        std::fs::create_dir_all(&path)?;
        std::fs::write(
            PathBuf::from(path).join("multiprocess.json"),
            serde_json::to_vec_pretty(&evidence)?,
        )?;
    }
    println!("SCREEN_STEP7_MULTIPROCESS_PASS {}", evidence["checks"]);
    Ok(())
}
