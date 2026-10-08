#[path = "common/project_test_config.rs"]
mod project_test_config;
use std::{panic::AssertUnwindSafe, time::Duration};
use futures_util::FutureExt;
use sqlx::{mysql::{MySqlConnectOptions,MySqlPoolOptions,MySqlSslMode},MySqlPool,Row};
use inxaiot_desk_buddy_lib::{
    application::aio_assets::UpdateAioNodeInput,
    domain::aio::inventory::{InventoryValues,WorkbenchNodeSnapshot},
    formal::{workbench_store::WorkbenchStore,resource_lease_repository::{ResourceLeaseRepository,LeaseRequest},operation_repository::{OperationRepository,OperationStart,OperationFinalResult,TargetFinalResult},aio_node_repository::ServiceVersionWrite},
    infrastructure::{aio_edit::save_registered,platform_aio::PlatformAioRepository,project_spaces,deployment_finalization::{AtomicDeploymentFinalization,AtomicTargetFinalization,finalize_deployment_atomically}},
};

async fn open(database: Option<&str>) -> MySqlPool {
    let config=project_test_config::database();
    let mut options=MySqlConnectOptions::new().host(&config.host).port(config.port).username(&config.username).password(&config.password).ssl_mode(MySqlSslMode::Disabled);
    if let Some(database)=database { options=options.database(database); }
    MySqlPoolOptions::new().max_connections(3).acquire_timeout(Duration::from_secs(10)).connect_with(options).await.expect("connect test database")
}
fn request(mac: &str, operation: &str, owner: &str) -> LeaseRequest {
    LeaseRequest { resource_type:"aio".into(),resource_key:mac.into(),domain_type:"aio".into(),operation_id:operation.into(),owner_instance_id:owner.into(),owner_user:"测试人员".into(),ttl:Duration::from_secs(60) }
}
async fn start_edit(pool: &MySqlPool, owner: &str) -> String {
    OperationRepository::new(pool.clone()).start(OperationStart {domain_type:"aio".into(),operation_type:"asset_edit".into(),operation_name:"编辑一体机资料".into(),operator_name:"测试人员".into(),instance_id:owner.into(),targets:vec![("aio".into(),"001122334455".into())],artifact_name:None,artifact_version:None,operation_summary:None,retry_of_operation_id:None}).await.unwrap().id
}

#[tokio::test]
#[ignore = "creates two isolated schemas on the authorized test server; copies table structure only and deletes its own schemas"]
async fn platform_edits_preserve_device_fields_and_successful_deployment_creates_only_result_assets() {
    let suffix=&uuid::Uuid::now_v7().simple().to_string()[..16];
    let business=format!("inxaiot_desk_buddy_edit_b_{suffix}");
    let shared=format!("inxaiot_desk_buddy_edit_w_{suffix}");
    let description=std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("test/测试数据说明.txt")).unwrap();
    let source=description.lines().find_map(|line| line.trim().strip_prefix("平台业务数据库名：")).unwrap().trim();
    assert!(source.bytes().all(|c| c.is_ascii_alphanumeric() || c==b'_'));
    let admin=open(None).await;
    for schema in [&business,&shared] { sqlx::query(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4")).execute(&admin).await.unwrap(); }
    let result=AssertUnwindSafe(async {
        let bp=open(Some(&business)).await; let wp=open(Some(&shared)).await;
        WorkbenchStore::new(wp.clone()).migrate().await.unwrap();
        for table in ["op_edge_aio_server","t_project_building"] {
            sqlx::query(&format!("CREATE TABLE `{business}`.`{table}` LIKE `{source}`.`{table}`")).execute(&admin).await.unwrap();
        }
        sqlx::query("INSERT INTO t_project_building(id,project_info_id,parent_id,area_name,area_level) VALUES(100,7,0,'回归项目',1),(101,7,100,'一号楼',3),(102,7,101,'一层',4),(103,7,101,'二层',4)").execute(&bp).await.unwrap();
        sqlx::query("INSERT INTO op_edge_aio_server(id,name,ip,mac,building_id,addr_alias,point_x,point_y,platform_ip,platform_port,account,password,status,last_beat_time) VALUES(10,'回归一体机','192.0.2.10','00:11:22:33:44:55',102,'旧位置','23','45','192.0.2.1','8055','fixture-user','fixture-unused',1,123)").execute(&bp).await.unwrap();
        sqlx::query("INSERT INTO aio_node(mac_normalized,name,ip,display_mac,building_id,addr_alias,management_state,source,version,created_at,updated_at) VALUES('001122334455','回归一体机','192.0.2.10','00:11:22:33:44:55','102','旧位置','managed','deployment',1,UTC_TIMESTAMP(6),UTC_TIMESTAMP(6))").execute(&wp).await.unwrap();
        let spaces=project_spaces::read(&bp,None).await.unwrap(); assert_eq!(spaces.len(),4);
        let original=PlatformAioRepository::new(bp.clone()).list_all().await.unwrap().nodes.remove(0);
        let input=UpdateAioNodeInput {mac:"001122334455".into(),expected_version:1,platform_base:Some(original.clone()),
            values:InventoryValues {name:"修改后的一体机".into(),ip:"192.0.2.11".into(),mac:"001122334455".into(),building_id:Some("103".into()),space_path:Some("回归项目/一号楼/二层".into()),addr_alias:Some("靠窗弱电柜".into()),..Default::default()},force_takeover:false};
        let leases=ResourceLeaseRepository::new(wp.clone());
        let edit_a=start_edit(&wp,"computer-a").await;
        let grant_a=leases.acquire_many(vec![request("001122334455",&edit_a,"computer-a")]).await.unwrap().remove(0);
        assert!(leases.acquire_many(vec![request("001122334455",&uuid::Uuid::now_v7().to_string(),"computer-b")]).await.is_err());
        let edit_b=start_edit(&wp,"computer-b").await;
        let grant_b=leases.force_acquire_many(vec![request("001122334455",&edit_b,"computer-b")]).await.unwrap().remove(0);
        assert!(save_registered(&bp,&shared,&input,"测试人员",&grant_a).await.is_err());
        save_registered(&bp,&shared,&input,"测试人员",&grant_b).await.unwrap();
        let actual=sqlx::query("SELECT name,ip,building_id,addr_alias,point_x,point_y,account,password,status,last_beat_time FROM op_edge_aio_server WHERE id=10").fetch_one(&bp).await.unwrap();
        assert_eq!(actual.get::<String,_>("name"),"修改后的一体机"); assert_eq!(actual.get::<String,_>("ip"),"192.0.2.11");
        assert_eq!(actual.get::<i64,_>("building_id"),103); assert_eq!(actual.get::<String,_>("addr_alias"),"靠窗弱电柜");
        assert_eq!(actual.get::<String,_>("point_x"),"23"); assert_eq!(actual.get::<String,_>("point_y"),"45");
        assert_eq!(actual.get::<String,_>("account"),"fixture-user"); assert_eq!(actual.get::<String,_>("password"),"fixture-unused");
        assert_eq!(actual.get::<i8,_>("status"),1); assert_eq!(actual.get::<u64,_>("last_beat_time"),123);
        let mirrored:(String,String)=sqlx::query_as("SELECT name,addr_alias FROM aio_node WHERE mac_normalized='001122334455'").fetch_one(&wp).await.unwrap();
        assert_eq!(mirrored,("修改后的一体机".into(),"靠窗弱电柜".into()));
        assert!(save_registered(&bp,&shared,&input,"测试人员",&grant_b).await.is_err());
        let current=PlatformAioRepository::new(bp.clone()).list_all().await.unwrap().nodes.remove(0);
        let edit_c=start_edit(&wp,"computer-a").await;
        assert!(leases.force_acquire_many(vec![request("001122334455",&edit_c,"computer-a")]).await.is_err());
        let grant=leases.takeover_for_new_operation(vec![request("001122334455",&edit_c,"computer-a")]).await.unwrap().remove(0);
        let mut invalid=input.clone(); invalid.platform_base=Some(current); invalid.values.building_id=Some("99999".into()); invalid.values.space_path=None;
        assert!(save_registered(&bp,&shared,&invalid,"测试人员",&grant).await.is_err());
        invalid.values.building_id=None; invalid.values.addr_alias=None;
        save_registered(&bp,&shared,&invalid,"测试人员",&grant).await.unwrap();
        let cleared:(i64,String)=sqlx::query_as("SELECT building_id,addr_alias FROM op_edge_aio_server WHERE id=10").fetch_one(&bp).await.unwrap();
        assert_eq!(cleared,(0,String::new())); leases.release(&grant).await.unwrap();
        let audit_count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE action='asset_edit'").fetch_one(&wp).await.unwrap(); assert_eq!(audit_count,2);

        // 初次部署前，共享库没有此待实施清单；成功结果一次性建立版本关联。
        let mac="001122334466";
        let operations=OperationRepository::new(wp.clone());
        let operation=operations.start(OperationStart {domain_type:"aio".into(),operation_type:"first_deploy".into(),operation_name:"首次部署".into(),operator_name:"测试人员".into(),instance_id:"computer-a".into(),targets:vec![("aio".into(),mac.into())],artifact_name:None,artifact_version:None,operation_summary:None,retry_of_operation_id:None}).await.unwrap();
        let grants=leases.acquire_many(vec![request(mac,&operation.id,"computer-a")]).await.unwrap();
        let before:i64=sqlx::query_scalar("SELECT COUNT(*) FROM aio_node WHERE mac_normalized=?").bind(mac).fetch_one(&wp).await.unwrap(); assert_eq!(before,0);
        let asset=WorkbenchNodeSnapshot {mac_normalized:mac.into(),name:"新部署设备".into(),ip:"192.0.2.20".into(),building_id:Some("103".into()),addr_alias:None,location:None,region_id:None,floor:None,remark:None,platform_aio_id:None,management_state:"pending".into(),source:"local".into(),last_operation_id:None,version:1};
        sqlx::query("INSERT INTO op_edge_aio_server(id,name,ip,mac,building_id,addr_alias,platform_ip,platform_port) VALUES(20,'新部署设备','192.0.2.20','00:11:22:33:44:66',0,'','192.0.2.1','8055')").execute(&bp).await.unwrap();
        let completion=inxaiot_desk_buddy_lib::infrastructure::aio_registration::RegistrationCompletion {pool:bp.clone(),shared_schema:shared.clone()};
        assert_eq!(completion.confirm(&asset,&grants[0]).await.unwrap(),"20");
        let registered:(i64,String)=sqlx::query_as("SELECT building_id,addr_alias FROM op_edge_aio_server WHERE id=20").fetch_one(&bp).await.unwrap();
        assert_eq!(registered,(103,String::new()));
        finalize_deployment_atomically(&wp,AtomicDeploymentFinalization {operation:OperationFinalResult {operation_id:operation.id.clone(),expected_version:operation.version,state:"succeeded".into(),result_summary:None,error_code:None,error_summary:None},leases:grants,
            targets:vec![AtomicTargetFinalization {asset:Some(asset),result:TargetFinalResult {operation_id:operation.id.clone(),resource_type:"aio".into(),resource_key:mac.into(),result_state:"succeeded".into(),before_version:None,after_version:None,result_summary:None,error_code:None,error_summary:None},replace_service_versions:true,mark_operation_success:true,
                service_versions:vec![ServiceVersionWrite {mac:mac.into(),service_name:"device-edge".into(),expected_image_name:Some("device-edge".into()),expected_version:Some("test".into()),observed_image_name:None,observed_version:None,source_operation_id:Some(operation.id.clone())}]}]}).await.unwrap();
        let after:(String,Option<String>)=sqlx::query_as("SELECT management_state,addr_alias FROM aio_node WHERE mac_normalized=?").bind(mac).fetch_one(&wp).await.unwrap();
        assert_eq!(after,("managed".into(),None));
        bp.close().await; wp.close().await;
    }).catch_unwind().await;
    for schema in [&business,&shared] {
        assert!(schema.starts_with("inxaiot_desk_buddy_edit_"));
        sqlx::query(&format!("DROP DATABASE `{schema}`")).execute(&admin).await.expect("remove this test schema");
    }
    admin.close().await;
    if let Err(error)=result { std::panic::resume_unwind(error); }
}
