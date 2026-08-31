use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client;
use russh::keys::key::PrivateKeyWithHashAlg;
use russh::keys::{HashAlg, PublicKeyOrCertificate, load_secret_key};
use russh::{ChannelMsg, Disconnect};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::core::error::{AppError, AppResult};

#[derive(Clone, Debug)]
pub struct SshConnectionConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub private_key_path: std::path::PathBuf,
    pub private_key_password: Option<String>,
    pub connect_timeout: Duration,
    pub inactivity_timeout: Duration,
}

impl SshConnectionConfig {
    fn validate(&self) -> AppResult<()> {
        if self.host.trim().is_empty() || self.username.trim().is_empty() {
            return Err(AppError::InvalidConfig("SSH主机或用户名不能为空".into()));
        }
        if self.port == 0 || !self.private_key_path.is_file() {
            return Err(AppError::InvalidConfig("SSH端口或私钥路径无效".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum HostKeyPolicy {
    Capture,
    RequireFingerprint(String),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCommandResult {
    pub exit_status: u32,
    pub stdout: String,
    pub stderr: String,
}

pub struct RemoteSession {
    handle: client::Handle<ClientHandler>,
    server_fingerprint: String,
}

#[derive(Clone)]
struct ClientHandler {
    policy: HostKeyPolicy,
    captured: Arc<Mutex<Option<String>>>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key
            .public_key()
            .fingerprint(HashAlg::Sha256)
            .to_string();
        if let Ok(mut captured) = self.captured.lock() {
            *captured = Some(fingerprint.clone());
        }
        Ok(match &self.policy {
            HostKeyPolicy::Capture => true,
            HostKeyPolicy::RequireFingerprint(expected) => expected == &fingerprint,
        })
    }
}

impl RemoteSession {
    pub async fn connect_private_key(
        config: &SshConnectionConfig,
        host_key_policy: HostKeyPolicy,
    ) -> AppResult<Self> {
        config.validate()?;
        let key = load_secret_key(
            &config.private_key_path,
            config.private_key_password.as_deref(),
        )
        .map_err(|error| AppError::ssh("读取SSH私钥", error))?;
        let captured = Arc::new(Mutex::new(None));
        let handler = ClientHandler {
            policy: host_key_policy,
            captured: captured.clone(),
        };
        let client_config = Arc::new(client::Config {
            inactivity_timeout: Some(config.inactivity_timeout),
            ..Default::default()
        });
        let mut handle = tokio::time::timeout(
            config.connect_timeout,
            client::connect(client_config, (config.host.as_str(), config.port), handler),
        )
        .await
        .map_err(|_| AppError::Ssh {
            operation: "建立SSH连接超时",
        })?
        .map_err(|error| AppError::ssh("建立SSH连接", error))?;

        let hash_algorithm = handle
            .best_supported_rsa_hash()
            .await
            .map_err(|error| AppError::ssh("协商RSA签名算法", error))?
            .flatten();
        let auth_result = handle
            .authenticate_publickey(
                &config.username,
                PrivateKeyWithHashAlg::new(Arc::new(key), hash_algorithm),
            )
            .await
            .map_err(|error| AppError::ssh("SSH私钥认证", error))?;
        if !auth_result.success() {
            return Err(AppError::Ssh {
                operation: "SSH私钥认证被拒绝",
            });
        }

        let server_fingerprint =
            captured
                .lock()
                .ok()
                .and_then(|value| value.clone())
                .ok_or(AppError::Ssh {
                    operation: "读取SSH主机指纹",
                })?;

        Ok(Self {
            handle,
            server_fingerprint,
        })
    }

    pub fn server_fingerprint(&self) -> &str {
        &self.server_fingerprint
    }

    pub async fn execute(&self, command: &str) -> AppResult<RemoteCommandResult> {
        if command.trim().is_empty() {
            return Err(AppError::InvalidConfig("远端命令不能为空".into()));
        }
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|error| AppError::ssh("打开SSH命令通道", error))?;
        channel
            .exec(true, command)
            .await
            .map_err(|error| AppError::ssh("启动远端命令", error))?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_status = None;
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus {
                    exit_status: status,
                } => exit_status = Some(status),
                _ => {}
            }
        }

        Ok(RemoteCommandResult {
            exit_status: exit_status.unwrap_or(255),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    }

    pub async fn upload_atomic<F>(
        &self,
        local_path: &Path,
        remote_path: &str,
        cancellation: &CancellationToken,
        mut on_progress: F,
    ) -> AppResult<()>
    where
        F: FnMut(u64, u64) + Send,
    {
        if !local_path.is_file() || !remote_path.starts_with('/') || remote_path.contains("..") {
            return Err(AppError::InvalidConfig("上传路径无效".into()));
        }
        let total = tokio::fs::metadata(local_path)
            .await
            .map_err(|error| AppError::io("读取上传文件信息", &error))?
            .len();
        let temporary_path = format!("{remote_path}.part");
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|error| AppError::ssh("打开SFTP通道", error))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| AppError::ssh("启动SFTP子系统", error))?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|error| AppError::sftp("初始化SFTP会话", error))?;
        let mut remote = sftp
            .open_with_flags(
                &temporary_path,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|error| AppError::sftp("创建远端临时文件", error))?;
        let mut local = tokio::fs::File::open(local_path)
            .await
            .map_err(|error| AppError::io("打开本地上传文件", &error))?;
        let mut buffer = vec![0_u8; 1024 * 1024];
        let mut transferred = 0_u64;

        loop {
            if cancellation.is_cancelled() {
                let _ = remote.shutdown().await;
                let _ = sftp.remove_file(&temporary_path).await;
                return Err(AppError::Cancelled);
            }
            let read = local
                .read(&mut buffer)
                .await
                .map_err(|error| AppError::io("读取本地上传文件", &error))?;
            if read == 0 {
                break;
            }
            remote
                .write_all(&buffer[..read])
                .await
                .map_err(|error| AppError::sftp("写入远端临时文件", error))?;
            transferred += read as u64;
            on_progress(transferred, total);
        }
        remote
            .flush()
            .await
            .map_err(|error| AppError::sftp("刷新远端临时文件", error))?;
        remote
            .shutdown()
            .await
            .map_err(|error| AppError::sftp("关闭远端临时文件", error))?;
        sftp.rename(&temporary_path, remote_path)
            .await
            .map_err(|error| AppError::sftp("原子发布远端文件", error))?;
        Ok(())
    }

    pub async fn disconnect(&self) -> AppResult<()> {
        self.handle
            .disconnect(Disconnect::ByApplication, "", "zh-CN")
            .await
            .map_err(|error| AppError::ssh("关闭SSH连接", error))
    }
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn shell_quote_handles_spaces_and_single_quotes() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }
}
