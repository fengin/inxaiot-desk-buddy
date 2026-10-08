use inxaiot_desk_buddy_lib::{domain::aio::inventory::{InventoryValues, PlatformNodeSnapshot, WorkbenchNodeSnapshot}, formal::local_store::LocalStore, infrastructure::{aio_inventory_source::merge_sources, local_sqlite::{aio_node_repository::LocalAioRepository, aio_import_repository::AioImportRepository}, csv_inventory::parse_inventory_text}};

async fn project(store: &LocalStore, id: &str) {
    sqlx::query("INSERT INTO local_project(id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) VALUES(?,?,'http://platform.test','db.test',3306,'user','business','workbench','ref','1','1')")
        .bind(id).bind(id).execute(store.pool()).await.unwrap();
}
fn values(mac: &str, ip: &str) -> InventoryValues {
    InventoryValues { name: "本机待实施设备".into(), ip: ip.into(), mac: mac.into(), building_id: Some("3".into()), addr_alias: Some("门口弱电柜".into()), location: Some("门口弱电柜".into()), ..Default::default() }
}

#[tokio::test]
async fn pending_inventory_is_computer_and_project_local_persistent_and_transactional() {
    let temp = tempfile::tempdir().unwrap();
    let a = LocalStore::open(temp.path().join("computer-a.db")).await.unwrap();
    let b = LocalStore::open(temp.path().join("computer-b.db")).await.unwrap();
    project(&a,"project").await; project(&a,"other").await; project(&b,"project").await;
    let local = LocalAioRepository::new(a.pool().clone());
    let sessions = AioImportRepository::new(a.pool().clone());
    let rows = parse_inventory_text("名称,IP,MAC,位置\n本机设备,192.0.2.1,001122334455,门口弱电柜\n").unwrap();
    let items = inxaiot_desk_buddy_lib::domain::aio::inventory::reconcile_inventory(rows, &[], &[]);
    let preview = sessions.create_preview("project", "local.csv", "test.csv", &items).await.unwrap();
    local.apply_import("project", &preview.id, &[(items[0].values.clone(),None)]).await.unwrap();
    assert_eq!(sessions.get(&preview.id).await.unwrap().state,"applied");
    assert!(local.apply_import("project", &preview.id, &[(items[0].values.clone(),None)]).await.is_err());
    assert!(local.list("other").await.unwrap().is_empty());
    assert!(LocalAioRepository::new(b.pool().clone()).list("project").await.unwrap().is_empty());
    let mut updated = values("001122334455","192.0.2.2"); updated.name="编辑后".into();
    local.save_many("project", &[(updated.clone(),Some(1))]).await.unwrap();
    assert!(local.save_many("project", &[(updated.clone(),Some(1))]).await.is_err());
    let other = values("001122334466","192.0.2.3");
    let duplicate = values("001122334477","192.0.2.2");
    assert!(local.save_many("project", &[(other,None),(duplicate,None)]).await.is_err());
    assert_eq!(local.list("project").await.unwrap().len(),1);
    drop(local); a.close().await;
    let reopened = LocalStore::open(temp.path().join("computer-a.db")).await.unwrap();
    let saved = LocalAioRepository::new(reopened.pool().clone()).list("project").await.unwrap();
    assert_eq!(saved[0].name,"编辑后"); assert_eq!(saved[0].building_id.as_deref(),Some("3"));
    assert_eq!(saved[0].addr_alias.as_deref(),Some("门口弱电柜"));
    assert_eq!(saved[0].version,2);
    reopened.close().await; b.close().await;
}

#[tokio::test]
async fn registration_uses_platform_fields_without_exposing_another_computers_pending_records() {
    let temp = tempfile::tempdir().unwrap(); let store=LocalStore::open(temp.path().join("local.db")).await.unwrap();
    project(&store,"project").await; project(&store,"other").await;
    let local=LocalAioRepository::new(store.pool().clone());
    let input=values("001122334455","192.0.2.1");
    local.save_many("project", &[(input.clone(),None)]).await.unwrap();
    local.save_many("other", &[(input,None)]).await.unwrap();
    let pending=local.list("project").await.unwrap();
    let mut foreign: WorkbenchNodeSnapshot=pending[0].clone(); foreign.mac_normalized="001122334466".into(); foreign.source="old_shared_import".into();
    let platform=PlatformNodeSnapshot { id:"77".into(),name:"平台注册后的名称".into(),ip:"192.0.2.7".into(),mac_raw:"00:11:22:33:44:55".into(),mac_normalized:"001122334455".into(),building_id:Some("9".into()),addr_alias:Some("新位置".into()),status:None,last_beat_time:None,last_sync_time:None };
    let merged=merge_sources(&pending,&[foreign],&[platform]);
    assert_eq!(merged.len(),1); assert_eq!(merged[0].name,"平台注册后的名称");
    assert_eq!(merged[0].platform_aio_id.as_deref(),Some("77"));
    assert_eq!(merged[0].addr_alias.as_deref(),Some("新位置"));
    local.retire("project","001122334455",1).await.unwrap();
    assert!(local.list("project").await.unwrap().is_empty());
    assert_eq!(local.list("other").await.unwrap().len(),1);
    store.close().await;
}

#[tokio::test]
async fn queued_deployment_prevents_local_identity_edits() {
    use inxaiot_desk_buddy_lib::{domain::common::task::TaskState, infrastructure::local_sqlite::task_repository::{TaskRepository,CreateTask}};
    let temp=tempfile::tempdir().unwrap(); let store=LocalStore::open(temp.path().join("local.db")).await.unwrap(); project(&store,"project").await;
    let local=LocalAioRepository::new(store.pool().clone()); let values=values("001122334455","192.0.2.1"); local.save_many("project",&[(values.clone(),None)]).await.unwrap();
    let tasks=TaskRepository::new(store.pool().clone());
    tasks.create(CreateTask {id:"task".into(),local_project_id:"project".into(),remote_operation_record_id:None,domain_type:"aio".into(),operation_type:"first_deploy".into(),name:"部署".into(),priority:0,batch_size:1,concurrency:1,payload_ref:None,log_path:temp.path().join("log.jsonl").to_string_lossy().into_owned(),targets:vec![("aio".into(),"001122334455".into())]}).await.unwrap();
    for (from,to) in [(TaskState::Draft,TaskState::Checking),(TaskState::Checking,TaskState::Ready),(TaskState::Ready,TaskState::Queued)] { tasks.transition("task",from,to,None,None).await.unwrap(); }
    assert!(local.save_many("project",&[(values,Some(1))]).await.unwrap_err().to_string().contains("排队"));
    assert_eq!(local.list("project").await.unwrap()[0].version,1);
    store.close().await;
}
