use std::path::Path;

pub struct DatabaseTestConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

pub fn database() -> DatabaseTestConfig {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project root");
    let text = std::fs::read_to_string(root.join("test/测试数据说明.txt")).expect("test data");
    let value = |label: &str| {
        text.lines()
            .find_map(|line| {
                line.trim()
                    .trim_start_matches('\u{feff}')
                    .strip_prefix(label)
            })
            .map(str::trim)
            .unwrap_or_else(|| panic!("测试说明缺少字段：{label}"))
    };
    let (username, password) = value("平台数据库账号密码：")
        .split_once('/')
        .expect("数据库凭据须使用账号/密码格式");
    DatabaseTestConfig {
        host: value("平台主机：").into(),
        port: std::env::var("INX_TEST_MYSQL_PORT")
            .ok()
            .map(|value| value.parse().expect("mysql port"))
            .unwrap_or(3306),
        username: username.into(),
        password: password.into(),
    }
}
