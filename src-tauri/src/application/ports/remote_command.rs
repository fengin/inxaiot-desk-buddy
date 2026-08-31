use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::core::error::{AppError, AppResult};

#[derive(Clone)]
pub struct ExecRequest {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub stdin: Option<Vec<u8>>,
    pub total_timeout: Duration,
    pub inactivity_timeout: Duration,
}

impl fmt::Debug for ExecRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExecRequest")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("env_keys", &self.env.keys().collect::<Vec<_>>())
            .field("stdin_bytes", &self.stdin.as_ref().map(Vec::len))
            .field("total_timeout", &self.total_timeout)
            .field("inactivity_timeout", &self.inactivity_timeout)
            .finish()
    }
}

impl ExecRequest {
    pub fn validate(&self) -> AppResult<()> {
        if self.program.trim().is_empty()
            || self.program.chars().any(char::is_control)
            || self.total_timeout.is_zero()
            || self.inactivity_timeout.is_zero()
            || self.inactivity_timeout > self.total_timeout
        {
            return Err(AppError::InvalidConfig("远端命令参数或超时无效".into()));
        }
        if self.env.keys().any(|key| !is_valid_env_key(key)) {
            return Err(AppError::InvalidConfig("远端命令环境变量名无效".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteOutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteOutputChunk {
    pub stream: RemoteOutputStream,
    pub text: String,
}

pub trait RemoteOutputSink: Send + Sync {
    fn emit(&self, chunk: RemoteOutputChunk) -> AppResult<()>;
}

#[derive(Default)]
pub struct NoopRemoteOutputSink;

impl RemoteOutputSink for NoopRemoteOutputSink {
    fn emit(&self, _chunk: RemoteOutputChunk) -> AppResult<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCommandResult {
    pub exit_status: u32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

#[allow(async_fn_in_trait)]
pub trait RemoteCommandExecutor: Send + Sync {
    async fn run(
        &self,
        request: &ExecRequest,
        cancellation: &CancellationToken,
        output: &dyn RemoteOutputSink,
    ) -> AppResult<RemoteCommandResult>;
}

pub fn is_valid_env_key(key: &str) -> bool {
    let mut characters = key.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}
