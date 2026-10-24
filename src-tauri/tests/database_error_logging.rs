use std::{
    borrow::Cow,
    error::Error,
    fmt,
    io::Write,
    sync::{Arc, Mutex},
};

use inxaiot_desk_buddy_lib::core::error::AppError;
use sqlx::{
    Connection,
    error::{DatabaseError, ErrorKind},
};

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn capture_error(error: &sqlx::Error) -> (String, String) {
    let bytes = Capture(Arc::new(Mutex::new(Vec::new())));
    let writer = bytes.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_max_level(tracing::Level::ERROR)
        .with_writer(move || writer.clone())
        .finish();
    let public = tracing::subscriber::with_default(subscriber, || {
        AppError::database("测试本机结果确认", error).to_string()
    });
    let log = String::from_utf8(bytes.0.lock().unwrap().clone()).unwrap();
    (log, public)
}

#[tokio::test]
async fn real_sqlite_extended_error_code_is_logged_without_sql_or_private_values() {
    let mut database = sqlx::SqliteConnection::connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE private_sentinel_table (private_password_column TEXT UNIQUE)")
        .execute(&mut database)
        .await
        .unwrap();
    sqlx::query("INSERT INTO private_sentinel_table VALUES(?)")
        .bind("PASSWORD_SENTINEL_DO_NOT_LOG")
        .execute(&mut database)
        .await
        .unwrap();
    let error = sqlx::query("INSERT INTO private_sentinel_table VALUES(?)")
        .bind("PASSWORD_SENTINEL_DO_NOT_LOG")
        .execute(&mut database)
        .await
        .unwrap_err();
    let code = error
        .as_database_error()
        .unwrap()
        .code()
        .unwrap()
        .into_owned();
    assert_eq!(code, "2067");
    let (log, public) = capture_error(&error);
    assert!(log.contains("error_kind=\"database\"") && log.contains("database_code=\"2067\""));
    assert_eq!(public, "数据库操作失败：测试本机结果确认");
    for secret in [
        "PASSWORD_SENTINEL",
        "private_sentinel_table",
        "private_password_column",
        "INSERT",
        "UNIQUE constraint failed",
    ] {
        assert!(!log.contains(secret), "日志不能包含原始SQL错误内容");
    }
}

#[test]
fn non_database_errors_log_only_static_categories() {
    let cases = [
        (sqlx::Error::PoolTimedOut, "pool_timed_out"),
        (sqlx::Error::PoolClosed, "pool_closed"),
        (
            sqlx::Error::Protocol("AUTH_SENTINEL password=never-print".into()),
            "protocol",
        ),
        (
            sqlx::Error::Io(std::io::Error::other("CREDENTIAL_SENTINEL")),
            "io",
        ),
        (
            sqlx::Error::Configuration("mysql://user:SECRET_SENTINEL@private/database".into()),
            "configuration",
        ),
        (
            sqlx::Error::ColumnNotFound("COLUMN_SENTINEL".into()),
            "column_not_found",
        ),
    ];
    for (error, expected) in cases {
        let (log, public) = capture_error(&error);
        assert!(log.contains(&format!("error_kind=\"{expected}\"")));
        assert!(log.contains("database_code=\"unavailable\""));
        assert!(!log.contains("SENTINEL"));
        assert_eq!(public, "数据库操作失败：测试本机结果确认");
    }
}

#[derive(Debug)]
struct SyntheticDatabaseError(&'static str);
impl fmt::Display for SyntheticDatabaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("password=DATABASE_SECRET_SENTINEL")
    }
}
impl Error for SyntheticDatabaseError {}
impl DatabaseError for SyntheticDatabaseError {
    fn message(&self) -> &str {
        "SELECT password FROM DATABASE_SECRET_SENTINEL"
    }
    fn code(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed(self.0))
    }
    fn as_error(&self) -> &(dyn Error + Send + Sync + 'static) {
        self
    }
    fn as_error_mut(&mut self) -> &mut (dyn Error + Send + Sync + 'static) {
        self
    }
    fn into_error(self: Box<Self>) -> Box<dyn Error + Send + Sync + 'static> {
        self
    }
    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}

#[test]
fn database_codes_are_bounded_and_do_not_accept_arbitrary_error_text() {
    for (code, expected) in [
        ("517", "517"),
        ("HY000", "HY000"),
        ("password=CODE_SECRET_SENTINEL", "unavailable"),
        ("12345678901", "unavailable"),
        ("", "unavailable"),
    ] {
        let error = sqlx::Error::Database(Box::new(SyntheticDatabaseError(code)));
        let (log, _) = capture_error(&error);
        assert!(log.contains(&format!("database_code=\"{expected}\"")));
        assert!(!log.contains("SENTINEL") && !log.contains("SELECT") && !log.contains("password="));
    }
}
