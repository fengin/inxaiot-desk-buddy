use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client;
use russh::keys::decode_secret_key;
use russh::keys::key::PrivateKeyWithHashAlg;
use russh::keys::{HashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::application::ports::file_transfer::{
    DownloadRequest, FileTransferService, RemoteFileMetadata, TransferProgress,
    TransferProgressSink, UploadRequest, validate_remote_path,
};
use crate::application::ports::remote_command::{
    ExecRequest, NoopRemoteOutputSink, RemoteCommandExecutor, RemoteCommandResult,
    RemoteOutputChunk, RemoteOutputSink, RemoteOutputStream,
};
pub use crate::application::ports::remote_session::{HostKeyIdentity, HostKeyPolicy};
use crate::application::ports::remote_session::{
    RemoteAuth, RemoteConnection, RemoteConnector, RemoteTarget,
};
use crate::core::error::{AppError, AppResult};

const DEFAULT_TRANSPORT_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(60);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Clone, Debug)]
pub struct RusshConnector {
    transport_inactivity_timeout: Duration,
}

impl Default for RusshConnector {
    fn default() -> Self {
        Self {
            transport_inactivity_timeout: DEFAULT_TRANSPORT_INACTIVITY_TIMEOUT,
        }
    }
}

impl RusshConnector {
    pub fn new(transport_inactivity_timeout: Duration) -> AppResult<Self> {
        if transport_inactivity_timeout.is_zero() {
            return Err(AppError::InvalidConfig("SSH传输空闲超时必须大于零".into()));
        }
        Ok(Self {
            transport_inactivity_timeout,
        })
    }
}

pub struct RemoteSession {
    handle: client::Handle<ClientHandler>,
    host_key: HostKeyIdentity,
}

#[derive(Clone)]
struct ClientHandler {
    policy: HostKeyPolicy,
    captured: Arc<Mutex<Option<HostKeyIdentity>>>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let public_key = server_public_key.public_key();
        let identity = HostKeyIdentity {
            algorithm: format!("{:?}", public_key.algorithm()),
            fingerprint: public_key.fingerprint(HashAlg::Sha256).to_string(),
        };
        if let Ok(mut captured) = self.captured.lock() {
            *captured = Some(identity.clone());
        }
        Ok(match &self.policy {
            HostKeyPolicy::Capture => true,
            HostKeyPolicy::Require(expected) => expected == &identity,
        })
    }
}

impl RemoteConnector for RusshConnector {
    type Connection = RemoteSession;

    async fn connect(
        &self,
        target: &RemoteTarget,
        auth: &RemoteAuth,
        host_key_policy: HostKeyPolicy,
    ) -> AppResult<Self::Connection> {
        target.validate()?;
        auth.validate()?;
        let captured = Arc::new(Mutex::new(None));
        let handler = ClientHandler {
            policy: host_key_policy.clone(),
            captured: captured.clone(),
        };
        let client_config = Arc::new(client::Config {
            inactivity_timeout: Some(self.transport_inactivity_timeout),
            ..Default::default()
        });
        let connection = tokio::time::timeout(
            target.connect_timeout,
            client::connect(client_config, (target.host.as_str(), target.port), handler),
        )
        .await
        .map_err(|_| AppError::timeout("建立SSH连接"))?;
        let mut handle = match connection {
            Ok(handle) => handle,
            Err(error) => {
                if let (HostKeyPolicy::Require(expected), Some(actual)) = (
                    host_key_policy,
                    captured.lock().ok().and_then(|value| value.clone()),
                ) && expected != actual
                {
                    return Err(AppError::HostKeyChanged {
                        expected: expected.fingerprint,
                        actual: actual.fingerprint,
                    });
                }
                return Err(AppError::ssh("建立SSH连接", error));
            }
        };

        let authenticated = match auth {
            RemoteAuth::Password { username, password } => handle
                .authenticate_password(username, password.expose())
                .await
                .map_err(|error| AppError::ssh("SSH密码认证", error))?,
            RemoteAuth::PrivateKey {
                username,
                private_key,
                passphrase,
            } => {
                let key = decode_secret_key(
                    private_key.expose(),
                    passphrase
                        .as_ref()
                        .map(crate::core::secret::SecretValue::expose),
                )
                .map_err(|error| AppError::ssh("解析SSH私钥", error))?;
                let hash_algorithm = handle
                    .best_supported_rsa_hash()
                    .await
                    .map_err(|error| AppError::ssh("协商RSA签名算法", error))?
                    .flatten();
                handle
                    .authenticate_publickey(
                        username,
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash_algorithm),
                    )
                    .await
                    .map_err(|error| AppError::ssh("SSH私钥认证", error))?
            }
        };
        if !authenticated.success() {
            return Err(AppError::Ssh {
                operation: "SSH认证被拒绝",
            });
        }
        let host_key =
            captured
                .lock()
                .ok()
                .and_then(|value| value.clone())
                .ok_or(AppError::Ssh {
                    operation: "读取SSH主机指纹",
                })?;
        Ok(RemoteSession { handle, host_key })
    }
}

impl RemoteConnection for RemoteSession {
    fn host_key(&self) -> &HostKeyIdentity {
        &self.host_key
    }

    async fn disconnect(&self) -> AppResult<()> {
        self.handle
            .disconnect(Disconnect::ByApplication, "", "zh-CN")
            .await
            .map_err(|error| AppError::ssh("关闭SSH连接", error))
    }
}

impl RemoteCommandExecutor for RemoteSession {
    async fn run(
        &self,
        request: &ExecRequest,
        cancellation: &CancellationToken,
        output: &dyn RemoteOutputSink,
    ) -> AppResult<RemoteCommandResult> {
        request.validate()?;
        if cancellation.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let started = Instant::now();
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|error| AppError::ssh("打开SSH命令通道", error))?;
        channel
            .exec(true, command_line(request))
            .await
            .map_err(|error| AppError::ssh("启动远端命令", error))?;
        if let Some(stdin) = &request.stdin {
            channel
                .data_bytes(stdin.clone())
                .await
                .map_err(|error| AppError::ssh("写入远端命令输入", error))?;
        }
        channel
            .eof()
            .await
            .map_err(|error| AppError::ssh("关闭远端命令输入", error))?;

        let total_deadline = Instant::now() + request.total_timeout;
        let inactivity = tokio::time::sleep(request.inactivity_timeout);
        tokio::pin!(inactivity);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_status = None;
        loop {
            tokio::select! {
                _ = cancellation.cancelled() => {
                    let _ = channel.close().await;
                    return Err(AppError::Cancelled);
                }
                _ = tokio::time::sleep_until(total_deadline) => {
                    let _ = channel.close().await;
                    return Err(AppError::timeout("远端命令总时长"));
                }
                _ = &mut inactivity => {
                    let _ = channel.close().await;
                    return Err(AppError::timeout("远端命令无输出"));
                }
                message = channel.wait() => {
                    let Some(message) = message else {
                        break;
                    };
                    inactivity.as_mut().reset(Instant::now() + request.inactivity_timeout);
                    match message {
                        ChannelMsg::Data { data } => {
                            stdout.extend_from_slice(&data);
                            output.emit(RemoteOutputChunk {
                                stream: RemoteOutputStream::Stdout,
                                text: String::from_utf8_lossy(&data).into_owned(),
                            })?;
                        }
                        ChannelMsg::ExtendedData { data, .. } => {
                            stderr.extend_from_slice(&data);
                            output.emit(RemoteOutputChunk {
                                stream: RemoteOutputStream::Stderr,
                                text: String::from_utf8_lossy(&data).into_owned(),
                            })?;
                        }
                        ChannelMsg::ExitStatus { exit_status: status } => {
                            exit_status = Some(status);
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(RemoteCommandResult {
            exit_status: exit_status.unwrap_or(255),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        })
    }
}

impl FileTransferService for RemoteSession {
    async fn upload(
        &self,
        request: &UploadRequest,
        cancellation: &CancellationToken,
        progress: &dyn TransferProgressSink,
    ) -> AppResult<()> {
        request.validate()?;
        let total = tokio::fs::metadata(&request.local_path)
            .await
            .map_err(|error| AppError::io("读取上传文件信息", &error))?
            .len();
        let total_timeout = transfer_timeout(
            total,
            request.minimum_bytes_per_second,
            request.minimum_total_timeout,
        );
        let temporary_path = request.temporary_remote_path();
        let sftp = self.open_sftp().await?;
        let transfer = async {
            let mut remote = sftp
                .open_with_flags(
                    &temporary_path,
                    OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
                )
                .await
                .map_err(|error| AppError::sftp("创建远端临时文件", error))?;
            let mut local = tokio::fs::File::open(&request.local_path)
                .await
                .map_err(|error| AppError::io("打开本地上传文件", &error))?;
            let mut buffer = vec![0_u8; request.chunk_size];
            let mut transferred = 0_u64;
            let mut last_progress = Instant::now() - PROGRESS_INTERVAL;
            loop {
                if cancellation.is_cancelled() {
                    return Err(AppError::Cancelled);
                }
                let read =
                    tokio::time::timeout(request.inactivity_timeout, local.read(&mut buffer))
                        .await
                        .map_err(|_| AppError::timeout("读取本地上传文件"))?
                        .map_err(|error| AppError::io("读取本地上传文件", &error))?;
                if read == 0 {
                    break;
                }
                tokio::time::timeout(
                    request.inactivity_timeout,
                    remote.write_all(&buffer[..read]),
                )
                .await
                .map_err(|_| AppError::timeout("写入远端临时文件"))?
                .map_err(|error| AppError::sftp("写入远端临时文件", error))?;
                transferred += read as u64;
                if transferred == total || last_progress.elapsed() >= PROGRESS_INTERVAL {
                    progress.emit(TransferProgress { transferred, total })?;
                    last_progress = Instant::now();
                }
            }
            remote
                .flush()
                .await
                .map_err(|error| AppError::sftp("刷新远端临时文件", error))?;
            remote
                .shutdown()
                .await
                .map_err(|error| AppError::sftp("关闭远端临时文件", error))?;
            let metadata = sftp
                .metadata(&temporary_path)
                .await
                .map_err(|error| AppError::sftp("读取远端临时文件信息", error))?;
            if metadata.size != Some(total) {
                return Err(AppError::integrity(
                    "远端文件大小不一致",
                    (metadata.size, total),
                ));
            }
            if let Some(expected) = &request.expected_sha256 {
                self.verify_remote_sha256(&temporary_path, expected, cancellation)
                    .await?;
            }
            if request.overwrite && sftp.metadata(&request.remote_path).await.is_ok() {
                sftp.remove_file(&request.remote_path)
                    .await
                    .map_err(|error| AppError::sftp("删除待覆盖远端文件", error))?;
            }
            sftp.rename(&temporary_path, &request.remote_path)
                .await
                .map_err(|error| AppError::sftp("原子发布远端文件", error))?;
            progress.emit(TransferProgress {
                transferred: total,
                total,
            })?;
            Ok(())
        };
        let result = tokio::time::timeout(total_timeout, transfer)
            .await
            .map_err(|_| AppError::timeout("SFTP上传总时长"))
            .and_then(|result| result);
        if result.is_err() {
            let _ = sftp.remove_file(&temporary_path).await;
        }
        result
    }

    async fn download(
        &self,
        request: &DownloadRequest,
        cancellation: &CancellationToken,
        progress: &dyn TransferProgressSink,
    ) -> AppResult<()> {
        request.validate()?;
        if request.local_path.exists() && !request.overwrite {
            return Err(AppError::Conflict(format!(
                "本地下载目标已存在：{}",
                request.local_path.display()
            )));
        }
        if let Some(parent) = request.local_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| AppError::io("创建下载目录", &error))?;
        }
        let temporary_path = request.temporary_local_path();
        if temporary_path.exists() {
            tokio::fs::remove_file(&temporary_path)
                .await
                .map_err(|error| AppError::io("清理本次下载临时文件", &error))?;
        }
        let sftp = self.open_sftp().await?;
        let metadata = sftp
            .metadata(&request.remote_path)
            .await
            .map_err(|error| AppError::sftp("读取远端下载文件信息", error))?;
        let total = metadata.size.ok_or(AppError::Sftp {
            operation: "远端下载文件缺少大小",
        })?;
        let total_timeout = transfer_timeout(
            total,
            request.minimum_bytes_per_second,
            request.minimum_total_timeout,
        );
        let transfer = async {
            let mut remote = sftp
                .open(&request.remote_path)
                .await
                .map_err(|error| AppError::sftp("打开远端下载文件", error))?;
            let mut local = tokio::fs::File::create(&temporary_path)
                .await
                .map_err(|error| AppError::io("创建本地下载临时文件", &error))?;
            let mut buffer = vec![0_u8; request.chunk_size];
            let mut transferred = 0_u64;
            let mut last_progress = Instant::now() - PROGRESS_INTERVAL;
            loop {
                if cancellation.is_cancelled() {
                    return Err(AppError::Cancelled);
                }
                let read =
                    tokio::time::timeout(request.inactivity_timeout, remote.read(&mut buffer))
                        .await
                        .map_err(|_| AppError::timeout("读取远端下载文件"))?
                        .map_err(|error| AppError::sftp("读取远端下载文件", error))?;
                if read == 0 {
                    break;
                }
                tokio::time::timeout(request.inactivity_timeout, local.write_all(&buffer[..read]))
                    .await
                    .map_err(|_| AppError::timeout("写入本地下载文件"))?
                    .map_err(|error| AppError::io("写入本地下载文件", &error))?;
                transferred += read as u64;
                if transferred == total || last_progress.elapsed() >= PROGRESS_INTERVAL {
                    progress.emit(TransferProgress { transferred, total })?;
                    last_progress = Instant::now();
                }
            }
            local
                .flush()
                .await
                .map_err(|error| AppError::io("刷新本地下载文件", &error))?;
            if transferred != total {
                return Err(AppError::integrity(
                    "下载文件大小不一致",
                    (transferred, total),
                ));
            }
            if let Some(expected) = &request.expected_sha256 {
                let actual = sha256_file(&temporary_path).await?;
                if !actual.eq_ignore_ascii_case(expected) {
                    return Err(AppError::integrity(
                        "下载文件SHA-256不一致",
                        (expected, actual),
                    ));
                }
            }
            if request.overwrite && request.local_path.exists() {
                tokio::fs::remove_file(&request.local_path)
                    .await
                    .map_err(|error| AppError::io("删除待覆盖本地文件", &error))?;
            }
            tokio::fs::rename(&temporary_path, &request.local_path)
                .await
                .map_err(|error| AppError::io("原子发布本地下载文件", &error))?;
            progress.emit(TransferProgress {
                transferred: total,
                total,
            })?;
            Ok(())
        };
        let result = tokio::time::timeout(total_timeout, transfer)
            .await
            .map_err(|_| AppError::timeout("SFTP下载总时长"))
            .and_then(|result| result);
        if result.is_err() && temporary_path.exists() {
            let _ = tokio::fs::remove_file(&temporary_path).await;
        }
        result
    }

    async fn stat(&self, remote_path: &str) -> AppResult<RemoteFileMetadata> {
        validate_remote_path(remote_path)?;
        let metadata = self
            .open_sftp()
            .await?
            .metadata(remote_path)
            .await
            .map_err(|error| AppError::sftp("读取远端文件信息", error))?;
        Ok(RemoteFileMetadata {
            size: metadata.size,
            permissions: metadata.permissions,
            modified_at_epoch_seconds: metadata.mtime,
            is_file: metadata.is_regular(),
            is_directory: metadata.is_dir(),
            is_symlink: metadata.is_symlink(),
        })
    }

    async fn rename(&self, source: &str, destination: &str) -> AppResult<()> {
        validate_remote_path(source)?;
        validate_remote_path(destination)?;
        self.open_sftp()
            .await?
            .rename(source, destination)
            .await
            .map_err(|error| AppError::sftp("重命名远端文件", error))
    }

    async fn remove_file(&self, remote_path: &str) -> AppResult<()> {
        validate_remote_path(remote_path)?;
        self.open_sftp()
            .await?
            .remove_file(remote_path)
            .await
            .map_err(|error| AppError::sftp("删除远端文件", error))
    }
}

impl RemoteSession {
    async fn open_sftp(&self) -> AppResult<SftpSession> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|error| AppError::ssh("打开SFTP通道", error))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| AppError::ssh("启动SFTP子系统", error))?;
        SftpSession::new(channel.into_stream())
            .await
            .map_err(|error| AppError::sftp("初始化SFTP会话", error))
    }

    async fn verify_remote_sha256(
        &self,
        remote_path: &str,
        expected: &str,
        cancellation: &CancellationToken,
    ) -> AppResult<()> {
        let request = ExecRequest {
            program: "sha256sum".into(),
            args: vec!["--".into(), remote_path.into()],
            env: Default::default(),
            stdin: None,
            total_timeout: Duration::from_secs(10 * 60),
            inactivity_timeout: Duration::from_secs(60),
        };
        let result = self
            .run(&request, cancellation, &NoopRemoteOutputSink)
            .await?;
        if result.exit_status != 0 {
            return Err(AppError::Integrity {
                operation: "远端SHA-256命令失败",
            });
        }
        let actual = result.stdout.split_whitespace().next().unwrap_or_default();
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(AppError::integrity(
                "远端文件SHA-256不一致",
                (expected, actual),
            ));
        }
        Ok(())
    }
}

fn command_line(request: &ExecRequest) -> String {
    let mut parts = request
        .env
        .iter()
        .map(|(key, value)| format!("{key}={}", shell_quote(value)))
        .collect::<Vec<_>>();
    parts.push("exec".into());
    parts.push(shell_quote(&request.program));
    parts.extend(request.args.iter().map(|argument| shell_quote(argument)));
    parts.join(" ")
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn transfer_timeout(total: u64, minimum_bps: u64, minimum_timeout: Duration) -> Duration {
    let budget_seconds = total.div_ceil(minimum_bps).saturating_add(30);
    minimum_timeout.max(Duration::from_secs(budget_seconds))
}

async fn sha256_file(path: &std::path::Path) -> AppResult<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| AppError::io("打开待校验文件", &error))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| AppError::io("读取待校验文件", &error))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hex::encode(hash.finalize()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use crate::application::ports::file_transfer::{DownloadRequest, UploadRequest};
    use crate::application::ports::remote_command::ExecRequest;

    use super::{command_line, shell_quote, transfer_timeout};

    #[test]
    fn shell_and_environment_values_are_safely_quoted() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        let request = ExecRequest {
            program: "agent.sh".into(),
            args: vec!["service check".into(), "a'b".into()],
            env: BTreeMap::from([("SAFE_NAME".into(), "value with space".into())]),
            stdin: None,
            total_timeout: Duration::from_secs(10),
            inactivity_timeout: Duration::from_secs(5),
        };
        assert_eq!(
            command_line(&request),
            "SAFE_NAME='value with space' exec 'agent.sh' 'service check' 'a'\\''b'"
        );
    }

    #[test]
    fn transfer_paths_and_timeout_are_operation_scoped() {
        let upload = UploadRequest {
            operation_id: "op-123".into(),
            local_path: std::env::current_exe().expect("current exe"),
            remote_path: "/opt/data/release.tar".into(),
            expected_sha256: None,
            overwrite: false,
            chunk_size: 1024,
            inactivity_timeout: Duration::from_secs(5),
            minimum_bytes_per_second: 1024,
            minimum_total_timeout: Duration::from_secs(30),
        };
        assert!(upload.validate().is_ok());
        assert_eq!(
            upload.temporary_remote_path(),
            "/opt/data/release.tar.part-op-123"
        );
        let download = DownloadRequest {
            operation_id: "op-123".into(),
            remote_path: "/opt/data/release.tar".into(),
            local_path: std::env::temp_dir().join("release.tar"),
            expected_sha256: None,
            overwrite: false,
            chunk_size: 1024,
            inactivity_timeout: Duration::from_secs(5),
            minimum_bytes_per_second: 1024,
            minimum_total_timeout: Duration::from_secs(30),
        };
        assert!(download.validate().is_ok());
        assert!(
            download
                .temporary_local_path()
                .to_string_lossy()
                .ends_with("release.tar.part-op-123")
        );
        assert_eq!(
            transfer_timeout(60 * 1024, 1024, Duration::from_secs(30)),
            Duration::from_secs(90)
        );
    }
}
