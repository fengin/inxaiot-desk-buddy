use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::error::{FormalError, FormalResult};

pub trait SecretStore: Send + Sync {
    fn save(&self, reference: &str, secret: &[u8]) -> FormalResult<()>;
    fn load(&self, reference: &str) -> FormalResult<Vec<u8>>;
    fn delete(&self, reference: &str) -> FormalResult<()>;
}

#[derive(Clone, Debug)]
pub struct OsSecretStore {
    service: String,
}

impl OsSecretStore {
    pub fn new(service: impl Into<String>) -> FormalResult<Self> {
        let service = service.into();
        if service.trim().is_empty() {
            return Err(FormalError::InvalidConfig("凭据服务名不能为空".into()));
        }
        Ok(Self { service })
    }

    fn entry(&self, reference: &str) -> FormalResult<keyring::Entry> {
        if reference.trim().is_empty() {
            return Err(FormalError::InvalidConfig("凭据引用不能为空".into()));
        }
        keyring::Entry::new(&self.service, reference).map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "create os secret entry failed");
            FormalError::SecretStore("创建本机凭据项")
        })
    }
}

impl SecretStore for OsSecretStore {
    fn save(&self, reference: &str, secret: &[u8]) -> FormalResult<()> {
        self.entry(reference)?.set_secret(secret).map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "save os secret failed");
            FormalError::SecretStore("保存本机凭据")
        })
    }

    fn load(&self, reference: &str) -> FormalResult<Vec<u8>> {
        match self.entry(reference)?.get_secret() {
            Ok(secret) => Ok(secret),
            Err(keyring::Error::NoEntry) => Err(FormalError::NotFound("本机凭据引用不存在".into())),
            Err(error) => {
                tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "load os secret failed");
                Err(FormalError::SecretStore("读取本机凭据"))
            }
        }
    }

    fn delete(&self, reference: &str) -> FormalResult<()> {
        match self.entry(reference)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => {
                tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "delete os secret failed");
                Err(FormalError::SecretStore("删除本机凭据"))
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct MemorySecretStore {
    values: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl SecretStore for MemorySecretStore {
    fn save(&self, reference: &str, secret: &[u8]) -> FormalResult<()> {
        self.values
            .lock()
            .map_err(|_| FormalError::SecretStore("锁定内存凭据存储"))?
            .insert(reference.to_string(), secret.to_vec());
        Ok(())
    }

    fn load(&self, reference: &str) -> FormalResult<Vec<u8>> {
        self.values
            .lock()
            .map_err(|_| FormalError::SecretStore("锁定内存凭据存储"))?
            .get(reference)
            .cloned()
            .ok_or_else(|| FormalError::NotFound("凭据引用不存在".into()))
    }

    fn delete(&self, reference: &str) -> FormalResult<()> {
        self.values
            .lock()
            .map_err(|_| FormalError::SecretStore("锁定内存凭据存储"))?
            .remove(reference);
        Ok(())
    }
}
