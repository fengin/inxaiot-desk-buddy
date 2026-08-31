use std::time::Duration;

use crate::application::ports::file_transfer::FileTransferService;
use crate::application::ports::remote_command::RemoteCommandExecutor;
use crate::core::error::AppResult;
use crate::core::secret::SecretValue;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteTarget {
    pub host: String,
    pub port: u16,
    pub connect_timeout: Duration,
}

impl RemoteTarget {
    pub fn validate(&self) -> AppResult<()> {
        if self.host.trim().is_empty() || self.port == 0 || self.connect_timeout.is_zero() {
            return Err(crate::core::error::AppError::InvalidConfig(
                "远程主机、端口或连接超时无效".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum RemoteAuth {
    Password {
        username: String,
        password: SecretValue,
    },
    PrivateKey {
        username: String,
        private_key: SecretValue,
        passphrase: Option<SecretValue>,
    },
}

impl RemoteAuth {
    pub fn username(&self) -> &str {
        match self {
            Self::Password { username, .. } | Self::PrivateKey { username, .. } => username,
        }
    }

    pub fn validate(&self) -> AppResult<()> {
        if self.username().trim().is_empty() {
            return Err(crate::core::error::AppError::InvalidConfig(
                "SSH用户名不能为空".into(),
            ));
        }
        let secret_is_empty = match self {
            Self::Password { password, .. } => password.is_empty(),
            Self::PrivateKey { private_key, .. } => private_key.is_empty(),
        };
        if secret_is_empty {
            return Err(crate::core::error::AppError::InvalidConfig(
                "SSH认证凭据不能为空".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyIdentity {
    pub algorithm: String,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyPolicy {
    Capture,
    Require(HostKeyIdentity),
}

#[allow(async_fn_in_trait)]
pub trait RemoteConnection: RemoteCommandExecutor + FileTransferService + Send + Sync {
    fn host_key(&self) -> &HostKeyIdentity;
    async fn disconnect(&self) -> AppResult<()>;
}

#[allow(async_fn_in_trait)]
pub trait RemoteConnector: Send + Sync {
    type Connection: RemoteConnection;

    async fn connect(
        &self,
        target: &RemoteTarget,
        auth: &RemoteAuth,
        host_key_policy: HostKeyPolicy,
    ) -> AppResult<Self::Connection>;
}
