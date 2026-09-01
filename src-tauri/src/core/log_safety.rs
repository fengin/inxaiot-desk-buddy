use std::any::{Any, type_name};

use crate::core::error::AppError;

pub struct SafeError<'a, T> {
    value: &'a T,
}

pub fn safe_error<T: Any>(value: &T) -> SafeError<'_, T> {
    SafeError { value }
}

impl<T: Any> std::fmt::Debug for SafeError<'_, T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut output = formatter.debug_struct("SafeError");
        output.field("type", &type_name::<T>());
        if let Some(detail) = safe_detail(self.value) {
            output.field("detail", &detail);
        }
        output.finish()
    }
}

impl<T: Any> std::fmt::Display for SafeError<'_, T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(type_name::<T>())?;
        if let Some(detail) = safe_detail(self.value) {
            write!(formatter, " [{detail}]")?;
        }
        Ok(())
    }
}

fn safe_detail<T: Any>(value: &T) -> Option<String> {
    let value = value as &dyn Any;
    if let Some(error) = value.downcast_ref::<AppError>() {
        return Some(app_error_detail(error));
    }
    if let Some(error) = value.downcast_ref::<std::io::Error>() {
        return Some(match error.raw_os_error() {
            Some(code) => format!("io_kind={:?}; os_code={code}", error.kind()),
            None => format!("io_kind={:?}", error.kind()),
        });
    }
    if let Some(error) = value.downcast_ref::<sqlx::Error>() {
        return Some(sqlx_error_detail(error));
    }
    if let Some(error) = value.downcast_ref::<reqwest::Error>() {
        let mut flags = Vec::new();
        if error.is_timeout() {
            flags.push("timeout");
        }
        if error.is_connect() {
            flags.push("connect");
        }
        if error.is_request() {
            flags.push("request");
        }
        if error.is_body() {
            flags.push("body");
        }
        if error.is_decode() {
            flags.push("decode");
        }
        let status = error
            .status()
            .map(|status| status.as_u16().to_string())
            .unwrap_or_else(|| "none".into());
        return Some(format!(
            "http_status={status}; class={}",
            if flags.is_empty() {
                "other".into()
            } else {
                flags.join(",")
            }
        ));
    }
    None
}

fn app_error_detail(error: &AppError) -> String {
    match error {
        AppError::InvalidConfig(_) => "code=invalid_config".into(),
        AppError::Conflict(_) => "code=conflict".into(),
        AppError::NotFound(_) => "code=not_found".into(),
        AppError::Database { operation } => format!("code=database; operation={operation}"),
        AppError::PlatformHttp { operation } => {
            format!("code=platform_http; operation={operation}")
        }
        AppError::Authentication(_) => "code=authentication".into(),
        AppError::Ssh { operation } => format!("code=ssh; operation={operation}"),
        AppError::Sftp { operation } => format!("code=sftp; operation={operation}"),
        AppError::Io { operation } => format!("code=io; operation={operation}"),
        AppError::Timeout { operation } => format!("code=timeout; operation={operation}"),
        AppError::HostKeyChanged { .. } => "code=host_key_changed".into(),
        AppError::Integrity { operation } => format!("code=integrity; operation={operation}"),
        AppError::Cancelled => "code=cancelled".into(),
    }
}

fn sqlx_error_detail(error: &sqlx::Error) -> String {
    match error {
        sqlx::Error::Database(database) => database
            .code()
            .map(|code| format!("sqlx=database; code={code}"))
            .unwrap_or_else(|| "sqlx=database".into()),
        sqlx::Error::Io(error) => match error.raw_os_error() {
            Some(code) => format!("sqlx=io; io_kind={:?}; os_code={code}", error.kind()),
            None => format!("sqlx=io; io_kind={:?}", error.kind()),
        },
        sqlx::Error::Tls(_) => "sqlx=tls".into(),
        sqlx::Error::PoolTimedOut => "sqlx=pool_timed_out".into(),
        sqlx::Error::PoolClosed => "sqlx=pool_closed".into(),
        sqlx::Error::WorkerCrashed => "sqlx=worker_crashed".into(),
        sqlx::Error::RowNotFound => "sqlx=row_not_found".into(),
        sqlx::Error::ColumnNotFound(_) => "sqlx=column_not_found".into(),
        sqlx::Error::ColumnIndexOutOfBounds { .. } => "sqlx=column_index_out_of_bounds".into(),
        sqlx::Error::ColumnDecode { .. } => "sqlx=column_decode".into(),
        sqlx::Error::Decode(_) => "sqlx=decode".into(),
        sqlx::Error::TypeNotFound { .. } => "sqlx=type_not_found".into(),
        sqlx::Error::Protocol(_) => "sqlx=protocol".into(),
        sqlx::Error::Configuration(_) => "sqlx=configuration".into(),
        sqlx::Error::AnyDriverError(_) => "sqlx=driver".into(),
        sqlx::Error::Migrate(_) => "sqlx=migrate".into(),
        _ => "sqlx=other".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::safe_error;
    use crate::core::error::AppError;

    #[test]
    fn safe_error_keeps_diagnostic_class_without_original_message() {
        let error = std::io::Error::other("password=top-secret");
        let debug = format!("{:?}", safe_error(&error));
        let display = safe_error(&error).to_string();
        assert!(debug.contains("io_kind=Other"));
        assert!(display.contains("io_kind=Other"));
        assert!(!debug.contains("top-secret"));
        assert!(!display.contains("top-secret"));
    }

    #[test]
    fn safe_error_keeps_stable_application_and_sqlx_details() {
        let application = safe_error(&AppError::Database {
            operation: "load project",
        })
        .to_string();
        assert!(application.contains("code=database"));
        assert!(application.contains("operation=load project"));

        let sqlx = safe_error(&sqlx::Error::PoolTimedOut).to_string();
        assert!(sqlx.contains("sqlx=pool_timed_out"));

        let protocol = safe_error(&sqlx::Error::Protocol("token=secret".into())).to_string();
        assert!(protocol.contains("sqlx=protocol"));
        assert!(!protocol.contains("secret"));
    }
}
