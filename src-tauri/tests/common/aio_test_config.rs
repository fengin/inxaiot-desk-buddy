#[path = "project_test_config.rs"]
mod project_test_config;

use inxaiot_desk_buddy_lib::core::secret::SecretValue;
use inxaiot_desk_buddy_lib::infrastructure::database::{DatabaseTlsMode, MySqlProjectConfig};
use std::path::Path;
use std::time::Duration;

pub fn isolated_project() -> MySqlProjectConfig {
    let database = project_test_config::database();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project");
    let description =
        std::fs::read_to_string(root.join("test/测试数据说明.txt")).expect("test data");
    let platform_schema = description
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("平台业务数据库名：")
                .map(str::trim)
        })
        .expect("platform schema");
    let workbench_schema = std::env::var("INX_AIO_TEST_SCHEMA")
        .expect("AIO real tests require an explicitly created INX_AIO_TEST_SCHEMA");
    assert!(workbench_schema.starts_with("inxaiot_desk_buddy_aio_regression_"));
    assert!(
        workbench_schema
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    );
    assert!(workbench_schema.len() <= 64);
    MySqlProjectConfig {
        host: database.host,
        port: database.port,
        username: database.username,
        password: SecretValue::new(database.password),
        platform_schema: platform_schema.into(),
        workbench_schema,
        tls_mode: DatabaseTlsMode::Disabled,
        connect_timeout: Duration::from_secs(10),
    }
}
