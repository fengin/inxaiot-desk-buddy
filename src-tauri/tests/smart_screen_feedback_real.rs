#[path = "common/project_test_config.rs"]
mod project_test_config;
use inxaiot_desk_buddy_lib::{formal::workbench_store::WorkbenchStore, infrastructure::smart_screen::{
    leases::HeldScreenLeases, lock_release, platform, platform_write, takeover,
    shared_results::{ScreenSharedResults,SharedScreenOperation}, write_context::ScreenWriteContext,
}};
use sqlx::ConnectOptions;

#[tokio::test]
#[ignore = "142 隔离双库验证人工释放、旧任务阻断、空间独立修改及删除确认；不写真实业务表"]
async fn confirmed_release_space_checks_and_deleted_assets() -> Result<(),Box<dyn std::error::Error>> {
    let cfg=project_test_config::database();
    let opts=sqlx::mysql::MySqlConnectOptions::new().host(&cfg.host).port(cfg.port).username(&cfg.username).password(&cfg.password)
        .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled).disable_statement_logging();
    let admin=sqlx::mysql::MySqlPoolOptions::new().max_connections(3).connect_with(opts.clone()).await?;
    let suffix=uuid::Uuid::now_v7().simple().to_string();
    let biz=format!("inxaiot_desk_buddy_feedback_b_{suffix}");
    let ops=format!("inxaiot_desk_buddy_feedback_w_{suffix}");
    for schema in [&biz,&ops] { sqlx::query(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4")).execute(&admin).await?; }
    let result:Result<(),Box<dyn std::error::Error>>=async {
        for table in ["smart_terminal_screen","t_project_building","op_device_service_area"] {
            sqlx::query(&format!("CREATE TABLE `{biz}`.{table} LIKE inxvision_iot_dev_demo.{table}")).execute(&admin).await?;
        }
        let business=sqlx::mysql::MySqlPoolOptions::new().max_connections(4).connect_with(opts.clone().database(&biz)).await?;
        let shared=sqlx::mysql::MySqlPoolOptions::new().max_connections(5).connect_with(opts.clone().database(&ops)).await?;
        // 第二个客户端用独立连接池，覆盖跨客户端的事务与旧占用检查。
        let second=sqlx::mysql::MySqlPoolOptions::new().max_connections(5).connect_with(opts.clone().database(&ops)).await?;
        WorkbenchStore::new(shared.clone()).migrate().await?;
        sqlx::query("INSERT INTO t_project_building(id,project_info_id,parent_id,area_name,area_level) VALUES(100,777,0,'旧空间',3),(101,777,0,'新空间',3),(102,888,0,'其他项目',3)").execute(&business).await?;
        sqlx::query("INSERT INTO smart_terminal_screen(id,name,ip,size,building_id,install_address) VALUES(900,'屏','192.0.2.1','4-inch',100,'测试')").execute(&business).await?;
        let source=platform::source_id(&business).await?;
        let context=|pool|ScreenWriteContext{business:"777".into(),source:source.clone(),read:business.clone(),write:business.clone(),shared:pool,shared_schema:ops.clone(),operator:"测试操作人".into()};
        let a=context(shared.clone());let b=context(second.clone());
        let op_a=uuid::Uuid::now_v7().to_string();let op_b=uuid::Uuid::now_v7().to_string();
        for (id,instance) in [(&op_a,"computer-a"),(&op_b,"computer-b")] {
            ScreenSharedResults::new(shared.clone()).start(&SharedScreenOperation{id,business_project_id:"777",action:"register",name:"测试",operator:"test",instance_id:instance,targets:&["777:900".into()],started_at:None}).await?;
        }
        let held_a=HeldScreenLeases::acquire(&a,&op_a,"computer-a",&["900".into()],true,false).await?;
        assert!(HeldScreenLeases::acquire(&b,&op_b,"computer-b",&["900".into()],true,false).await.is_err());
        let preview=lock_release::preview_with_context(&b,&op_a).await?;assert_eq!(preview.len(),2);
        let conflict=takeover::conflicts(&b,&[("smart_screen".into(),"777:900".into())]).await?;
        assert_eq!(conflict.len(),1);assert_eq!(conflict[0].operation_id,op_a);
        assert_eq!(conflict[0].target_count,1);
        assert_eq!(conflict[0].owner_instance_id,"computer-a");assert!(conflict[0].targets[0].contains("192.0.2.1"));
        assert!(takeover::conflicts(&b,&[("smart_screen".into(),"777:other".into())]).await?.is_empty());
        assert!(takeover::release_with_context(&b,&conflict,false,"computer-b").await.is_err());
        assert!(lock_release::release_with_context(&b,&op_a,&preview,false,"computer-b").await.is_err());
        let mut stale=preview.clone();stale[0].fencing_token+=1;
        assert!(lock_release::release_with_context(&b,&op_a,&stale,true,"computer-b").await.is_err());
        let mut wrong=context(second.clone());wrong.business="888".into();
        assert!(lock_release::release_with_context(&wrong,&op_a,&preview,true,"computer-b").await.is_err());
        // 多个原操作必须一起核对；任一确认已过期，前面释放的也要回滚。
        let op_c=uuid::Uuid::now_v7().to_string();
        ScreenSharedResults::new(shared.clone()).start(&SharedScreenOperation{id:&op_c,business_project_id:"777",action:"reboot",name:"重启屏",operator:"test-c",instance_id:"computer-c",targets:&["777:901".into()],started_at:None}).await?;
        let held_c=HeldScreenLeases::acquire(&a,&op_c,"computer-c",&["901".into()],false,false).await?;
        let mut multiple=takeover::conflicts(&b,&[("smart_screen".into(),"777:900".into()),("smart_screen".into(),"777:901".into())]).await?;
        assert_eq!(multiple.len(),2);multiple.last_mut().unwrap().locks[0].fencing_token+=1;
        assert!(takeover::release_with_context(&b,&multiple,true,"computer-b").await.is_err());
        held_a.valid().await?;held_c.valid().await?;
        let finished=takeover::conflicts(&b,&[("smart_screen".into(),"777:901".into())]).await?;
        held_c.release().await?;
        takeover::release_with_context(&b,&finished,true,"computer-b").await?;
        takeover::release_with_context(&b,&conflict,true,"computer-b").await?;
        assert!(held_a.valid().await.is_err());
        assert!(HeldScreenLeases::acquire(&a,&op_a,"computer-a",&["900".into()],true,true).await.is_err());
        let held_b=HeldScreenLeases::acquire(&b,&op_b,"computer-b",&["900".into()],true,false).await?;
        let audit:i64=sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE action='force_release'").fetch_one(&shared).await?;assert_eq!(audit,2);
        // 两个空间都有配置且屏已有坐标，仍允许独立修改所属空间。
        sqlx::query("UPDATE t_project_building SET building_image='image',device_cat='[1]',screen_version='full',screen_style_type=1 WHERE id IN (100,101)").execute(&business).await?;
        sqlx::query("UPDATE smart_terminal_screen SET point_x='12',point_y='34' WHERE id=900").execute(&business).await?;
        let old=platform_write::record(&business,"900").await?.unwrap().asset.fields;
        let mut next=old.clone();next.space_id=Some("101".into());
        sqlx::query("INSERT INTO op_device_service_area(building_id,layout_building_id,device_id,device_alias_name,point_x) VALUES(100,101,1,'灯','10')").execute(&business).await?;
        assert!(platform_write::update(&a,&held_a.grants,"900",&old,&next,None,false).await.is_err());
        platform_write::update(&b,&held_b.grants,"900",&old,&next,None,false).await.map_err(|e|e.message())?;
        let actual:(String,String,String)=sqlx::query_as("SELECT CAST(building_id AS CHAR),point_x,point_y FROM smart_terminal_screen WHERE id=900").fetch_one(&business).await?;
        assert_eq!(actual,("101".into(),"12".into(),"34".into()));
        let configurations:i64=sqlx::query_scalar("SELECT COUNT(*) FROM t_project_building WHERE id IN (100,101) AND building_image='image' AND device_cat='[1]' AND screen_version='full' AND screen_style_type=1").fetch_one(&business).await?;assert_eq!(configurations,2);
        let aliases:i64=sqlx::query_scalar("SELECT COUNT(*) FROM op_device_service_area WHERE building_id=100 AND layout_building_id=101 AND device_alias_name='灯' AND point_x='10'").fetch_one(&business).await?;assert_eq!(aliases,1);
        // 移除关联拦截不放宽空间有效性、项目归属和并发修改检查。
        for invalid in ["102","999"] {
            let mut rejected=next.clone();rejected.space_id=Some(invalid.into());
            assert!(platform_write::update(&b,&held_b.grants,"900",&next,&rejected,None,false).await.is_err());
        }
        sqlx::query("UPDATE t_project_building SET delete_flag='1' WHERE id=100").execute(&business).await?;
        assert!(platform_write::update(&b,&held_b.grants,"900",&next,&old,None,false).await.is_err());
        sqlx::query("UPDATE t_project_building SET delete_flag='0' WHERE id=100").execute(&business).await?;
        sqlx::query("INSERT INTO t_project_building(id,project_info_id,parent_id,area_name) VALUES(103,777,0,'第三空间')").execute(&business).await?;
        let mut stale=old.clone();stale.space_id=Some("103".into());
        assert!(platform_write::update(&b,&held_b.grants,"900",&old,&stale,None,false).await.is_err());
        held_b.release().await?;drop(held_a);
        // 移往其他业务项目不等于删除；逻辑删除和物理删除才清理。
        sqlx::query("UPDATE smart_terminal_screen SET building_id=102 WHERE id=900").execute(&business).await?;
        let (assets,_,deleted)=platform::read_with_known_ids(&business,"777",&["900".into()]).await?;
        assert!(assets.is_empty()&&deleted.is_empty());
        sqlx::query("UPDATE smart_terminal_screen SET delete_flag=1 WHERE id=900").execute(&business).await?;
        assert_eq!(platform::read_with_known_ids(&business,"777",&["900".into()]).await?.2,vec!["900"]);
        sqlx::query("DELETE FROM smart_terminal_screen WHERE id=900").execute(&business).await?;
        assert_eq!(platform::read_with_known_ids(&business,"777",&["900".into()]).await?.2,vec!["900"]);
        second.close().await;shared.close().await;business.close().await;
        Ok(())
    }.await;
    for schema in [&ops,&biz] { sqlx::query(&format!("DROP DATABASE `{schema}`")).execute(&admin).await?; }
    admin.close().await;result
}
