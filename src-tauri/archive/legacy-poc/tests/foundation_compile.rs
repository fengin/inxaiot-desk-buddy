#[path = "../src/core/mod.rs"]
mod core;
#[path = "../src/infrastructure/mod.rs"]
mod infrastructure;

use std::time::Duration;

use core::secret::SecretValue;
use infrastructure::database::MySqlProjectConfig;
use infrastructure::platform_auth::PlatformLoginConfig;

#[test]
fn formal_adapters_have_redacted_configuration_types() {
    let database = MySqlProjectConfig {
        host: "127.0.0.1".into(),
        port: 3306,
        username: "tester".into(),
        password: SecretValue::new("database-secret"),
        platform_schema: "platform_db".into(),
        workbench_schema: "workbench_db".into(),
        connect_timeout: Duration::from_secs(5),
    };
    let login = PlatformLoginConfig {
        base_url: "127.0.0.1:8055".into(),
        principal: "tester".into(),
        credentials: SecretValue::new("login-secret"),
        session_uuid: SecretValue::new("session-secret"),
        image_code: SecretValue::new("111"),
        timeout: Duration::from_secs(5),
    };

    assert!(!format!("{database:?}").contains("database-secret"));
    assert!(!format!("{login:?}").contains("login-secret"));
}

