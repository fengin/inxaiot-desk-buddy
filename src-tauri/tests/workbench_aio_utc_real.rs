#[path = "common/project_test_config.rs"]
mod project_test_config;

use std::io::Error;

use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use inxaiot_desk_buddy_lib::infrastructure::workbench_aio::WorkbenchAioRepository;
use sqlx::{mysql::MySqlConnectOptions, ConnectOptions, MySqlPool};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const PREFIX: &str = "inxaiot_desk_buddy_time_";
const FIXED: &str = "2026-10-21 05:39:11.123456";
const EXPECTED: &str = "2026-10-21T05:39:11Z";
const MAC_WITH_TIME: &str = "AABBCCDDEE01";
const MAC_WITH_NULL: &str = "AABBCCDDEE02";

fn require(condition: bool, message: &'static str) -> TestResult<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::other(message).into())
    }
}

fn valid_name(schema: &str, nonce: &str) -> bool {
    nonce.len() == 32
        && nonce
            .bytes()
            .all(|value| value.is_ascii_hexdigit() && !value.is_ascii_uppercase())
        && schema == format!("{PREFIX}{nonce}")
}

async fn exercise(pool: &MySqlPool) -> TestResult<serde_json::Value> {
    WorkbenchStore::new(pool.clone()).migrate().await?;
    for (mac, observed, operation, ended) in [
        (
            MAC_WITH_TIME,
            Some(FIXED),
            "00000000-0000-4000-8000-000000000001",
            Some(FIXED),
        ),
        (
            MAC_WITH_NULL,
            None,
            "00000000-0000-4000-8000-000000000002",
            None,
        ),
    ] {
        sqlx::query("INSERT INTO aio_node(mac_normalized,name,ip,display_mac,management_state,source,created_at,updated_at) VALUES(?,'UTC读出回归','192.0.2.1',?,'managed','test',?,?)")
            .bind(mac).bind(mac).bind(FIXED).bind(FIXED).execute(pool).await
            .map_err(|_| Error::other("写入独立测试资产失败"))?;
        sqlx::query("INSERT INTO aio_node_service_version(mac_normalized,service_name,observed_at) VALUES(?,'test-service',?)")
            .bind(mac).bind(observed).execute(pool).await
            .map_err(|_| Error::other("写入独立测试服务时间失败"))?;
        sqlx::query("INSERT INTO operation_record(id,domain_type,operation_type,operation_name,operator_name,instance_id,state,target_count,success_count,started_at,ended_at,heartbeat_at) VALUES(?,'aio','test','UTC读出回归','test','test','succeeded',1,1,?,?,?)")
            .bind(operation).bind(FIXED).bind(ended).bind(FIXED).execute(pool).await
            .map_err(|_| Error::other("写入独立测试操作时间失败"))?;
        sqlx::query("INSERT INTO operation_target_result(operation_id,resource_type,resource_key,result_state) VALUES(?,'aio',?,'succeeded')")
            .bind(operation).bind(mac).execute(pool).await
            .map_err(|_| Error::other("写入独立测试操作关联失败"))?;
    }
    let repository = WorkbenchAioRepository::new(pool.clone());
    let services = repository.list_service_versions().await?;
    let operations = repository.list_last_operations().await?;
    require(
        services.len() == 2 && operations.len() == 2,
        "真实Repository返回数量不符",
    )?;
    let service_time = services
        .iter()
        .find(|row| row.mac_normalized == MAC_WITH_TIME)
        .ok_or_else(|| Error::other("缺少固定UTC服务观测"))?
        .observed_at
        .as_deref();
    let service_null = services
        .iter()
        .find(|row| row.mac_normalized == MAC_WITH_NULL)
        .ok_or_else(|| Error::other("缺少NULL服务观测"))?
        .observed_at
        .as_deref();
    let operation_time = operations
        .get(MAC_WITH_TIME)
        .ok_or_else(|| Error::other("缺少固定UTC最近操作"))?
        .ended_at
        .as_deref();
    let operation_null = operations
        .get(MAC_WITH_NULL)
        .ok_or_else(|| Error::other("缺少NULL最近操作"))?
        .ended_at
        .as_deref();
    require(
        service_time == Some(EXPECTED),
        "服务观测没有按原秒精度明确UTC",
    )?;
    require(
        operation_time == Some(EXPECTED),
        "最近操作没有按原秒精度明确UTC",
    )?;
    require(
        service_null.is_none() && operation_null.is_none(),
        "UTC投影改变了NULL语义",
    )?;
    for value in [service_time, operation_time].into_iter().flatten() {
        let parsed = OffsetDateTime::parse(value, &Rfc3339)?;
        require(
            parsed.unix_timestamp() == 1_792_561_151 && parsed.nanosecond() == 0,
            "UTC投影改变了时刻或原有秒精度",
        )?;
    }
    let stored_service: Option<String> = sqlx::query_scalar("SELECT DATE_FORMAT(observed_at,'%Y-%m-%d %H:%i:%s.%f') FROM aio_node_service_version WHERE mac_normalized=?")
        .bind(MAC_WITH_TIME).fetch_one(pool).await
        .map_err(|_| Error::other("核验存储服务时间失败"))?;
    let stored_operation: Option<String> = sqlx::query_scalar("SELECT DATE_FORMAT(ended_at,'%Y-%m-%d %H:%i:%s.%f') FROM operation_record WHERE id='00000000-0000-4000-8000-000000000001'")
        .fetch_one(pool).await.map_err(|_| Error::other("核验存储操作时间失败"))?;
    require(
        stored_service.as_deref() == Some(FIXED) && stored_operation.as_deref() == Some(FIXED),
        "接口投影影响了数据库原有微秒值",
    )?;
    Ok(
        serde_json::json!({"serviceObservedAt":service_time,"lastOperationEndedAt":operation_time,
        "nullServiceObservedAt":service_null,"nullLastOperationEndedAt":operation_null,
        "storedMicrosecondsPreserved":true,"outputPrecision":"seconds","epochSeconds":1_792_561_151}),
    )
}

async fn cleanup(admin: &MySqlPool, schema: &str, nonce: &str) -> TestResult<()> {
    require(
        valid_name(schema, nonce),
        "拒绝清理：库名超出本轮随机前缀和nonce",
    )?;
    let markers: Vec<String> =
        sqlx::query_scalar(&format!("SELECT nonce FROM `{schema}`.codex_time_fixture"))
            .fetch_all(admin)
            .await
            .map_err(|_| Error::other("拒绝清理：无法核验本轮数据库标记"))?;
    require(markers == [nonce], "拒绝清理：本轮数据库标记不匹配")?;
    sqlx::query(&format!("DROP DATABASE `{schema}`"))
        .execute(admin)
        .await
        .map_err(|_| Error::other("清理本轮独立时间测试库失败"))?;
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name=?")
            .bind(schema)
            .fetch_one(admin)
            .await
            .map_err(|_| Error::other("核验本轮测试库已删除失败"))?;
    require(remaining == 0, "本轮时间测试库清理后仍存在")
}

#[tokio::test]
#[ignore = "仅在已授权测试MySQL创建带nonce的独立时间测试库，验证后严格清理"]
async fn utc_projections_preserve_instants_nulls_and_storage() -> TestResult<()> {
    let cfg = project_test_config::database();
    require(
        cfg.host == "192.168.3.142",
        "当前测试说明不是已授权142测试主机",
    )?;
    let nonce = uuid::Uuid::now_v7().simple().to_string();
    let schema = format!("{PREFIX}{nonce}");
    require(valid_name(&schema, &nonce), "本轮随机库名不合法")?;
    let options = MySqlConnectOptions::new()
        .host(&cfg.host)
        .port(cfg.port)
        .username(&cfg.username)
        .password(&cfg.password)
        .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled)
        .disable_statement_logging();
    let admin = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await
        .map_err(|_| Error::other("连接已授权测试MySQL失败，未输出认证信息"))?;
    let exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name=?")
            .bind(&schema)
            .fetch_one(&admin)
            .await
            .map_err(|_| Error::other("检查随机测试库名称失败"))?;
    require(exists == 0, "随机测试库已经存在，拒绝覆盖")?;
    sqlx::query(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4"))
        .execute(&admin)
        .await
        .map_err(|_| Error::other("创建独立时间测试库失败"))?;
    eprintln!("TIME_FIXTURE_CREATED schema={schema} nonce={nonce}");
    sqlx::query(&format!(
        "CREATE TABLE `{schema}`.codex_time_fixture(nonce CHAR(32) NOT NULL PRIMARY KEY)"
    ))
    .execute(&admin)
    .await
    .map_err(|_| Error::other("创建时间测试标记失败，保留库名待核验"))?;
    sqlx::query(&format!(
        "INSERT INTO `{schema}`.codex_time_fixture(nonce) VALUES(?)"
    ))
    .bind(&nonce)
    .execute(&admin)
    .await
    .map_err(|_| Error::other("写入时间测试标记失败，保留库名待核验"))?;
    // 断言使用Result而非panic；迁移、读写或断言失败后仍先核标记并清理。
    let checked = async {
        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(2)
            .connect_with(options.database(&schema))
            .await
            .map_err(|_| Error::other("连接本轮独立时间测试库失败"))?;
        let result = exercise(&pool).await;
        pool.close().await;
        result
    }
    .await;
    let cleaned = cleanup(&admin, &schema, &nonce).await;
    admin.close().await;
    let evidence = serde_json::json!({"schema":schema,"nonce":nonce,"preexistingSchema":false,
        "testPassed":checked.is_ok(),"markerCheckedCleanupSucceeded":cleaned.is_ok(),
        "repositoryEvidence":checked.as_ref().ok(),"productionTablesRead":false,
        "deviceAccessPerformed":false,"productionDirectoryAccessed":false});
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".review-tools/time-local-display-20261021");
    std::fs::create_dir_all(&directory)?;
    std::fs::write(
        directory.join(format!("aio-utc-real-{nonce}.json")),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    eprintln!("TIME_FIXTURE_RESULT {evidence}");
    cleaned?;
    checked?;
    Ok(())
}
