use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::Argon2;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use time::OffsetDateTime;
use tokio::sync::Mutex;
use zeroize::{Zeroize, Zeroizing};

use super::credential_crypto::{CredentialMetadata, ProjectMasterKey};
use super::error::{FormalError, FormalResult};
use super::project_repository::LocalProjectRepository;
use super::release_profile_repository::{
    ReleaseProfileRecord, ReleaseProfileRepository, ReleaseProfileWrite,
};
use super::secret_store::SecretStore;

static RELEASE_KEY_OPERATION_LOCK: Mutex<()> = Mutex::const_new(());
const TRANSFER_FORMAT: &str = "inxaiot-release-master-key-v1";
const TRANSFER_KDF: &str = "argon2id";
const TRANSFER_CIPHER: &str = "aes-256-gcm";
const TRANSFER_AAD_PREFIX: &str = "inxaiot-desk-buddy:release-master-key-transfer:v1";
const MAX_TRANSFER_FILE_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseKeyProjectBinding {
    pub platform_url: String,
    pub db_host: String,
    pub db_port: u16,
    pub workbench_db: String,
}

impl ReleaseKeyProjectBinding {
    pub fn fingerprint(&self) -> FormalResult<String> {
        if self.platform_url.trim().is_empty()
            || self.db_host.trim().is_empty()
            || self.db_port == 0
            || self.workbench_db.trim().is_empty()
        {
            return Err(FormalError::InvalidConfig(
                "项目主密钥绑定参数不完整".into(),
            ));
        }
        let canonical = format!(
            "{}|{}|{}|{}",
            self.platform_url
                .trim()
                .trim_end_matches('/')
                .to_ascii_lowercase(),
            self.db_host.trim().to_ascii_lowercase(),
            self.db_port,
            self.workbench_db.trim().to_ascii_lowercase(),
        );
        Ok(hex::encode(Sha256::digest(canonical.as_bytes())))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EncryptedKeyPackage {
    format: String,
    project_binding: String,
    key_version: u32,
    kdf: String,
    cipher: String,
    salt: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Clone)]
pub struct ReleaseMasterKeyManager {
    secrets: Arc<dyn SecretStore>,
    local_pool: Option<SqlitePool>,
}

impl ReleaseMasterKeyManager {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            secrets,
            local_pool: None,
        }
    }

    pub fn with_local_registry(secrets: Arc<dyn SecretStore>, local_pool: SqlitePool) -> Self {
        Self {
            secrets,
            local_pool: Some(local_pool),
        }
    }

    pub async fn load_profile(
        &self,
        repository: &ReleaseProfileRepository,
        project_id: &str,
        profile_key: &str,
    ) -> FormalResult<ReleaseProfileRecord> {
        let _guard = RELEASE_KEY_OPERATION_LOCK.lock().await;
        let metadata = repository
            .credential_metadata(profile_key)
            .await?
            .ok_or_else(|| FormalError::NotFound(format!("发布配置不存在：{profile_key}")))?;
        let key = self.resolve_key_locked(project_id, metadata).await?;
        repository.get(&key, profile_key).await
    }

    pub async fn save_profile(
        &self,
        repository: &ReleaseProfileRepository,
        project_id: &str,
        write: ReleaseProfileWrite,
    ) -> FormalResult<ReleaseProfileRecord> {
        let _guard = RELEASE_KEY_OPERATION_LOCK.lock().await;
        let metadata = repository.credential_metadata(&write.profile_key).await?;
        let (key, created) = match metadata {
            Some(metadata) => (self.resolve_key_locked(project_id, metadata).await?, false),
            None => self.ensure_key(project_id, 1).await?,
        };
        let result = repository.save(&key, write).await;
        if result.is_err() && created {
            let _ = self.delete_key_tracked(project_id, key.version()).await;
        }
        result
    }

    pub async fn export_key_package(
        &self,
        repository: &ReleaseProfileRepository,
        project_id: &str,
        profile_key: &str,
        binding: &ReleaseKeyProjectBinding,
        file_path: &Path,
        passphrase: &str,
    ) -> FormalResult<u32> {
        let _guard = RELEASE_KEY_OPERATION_LOCK.lock().await;
        let metadata = repository
            .credential_metadata(profile_key)
            .await?
            .ok_or_else(|| FormalError::NotFound(format!("发布配置不存在：{profile_key}")))?;
        let key = self.resolve_key_locked(project_id, metadata).await?;
        let project_binding = binding.fingerprint()?;
        let package = encrypt_transfer_package(&key, &project_binding, passphrase)?;
        let contents = serde_json::to_vec_pretty(&package)
            .map_err(|_| FormalError::InvalidConfig("序列化项目主密钥包失败".into()))?;
        write_new_file_atomically(file_path, &contents)?;
        Ok(key.version())
    }

    pub async fn import_key_package(
        &self,
        repository: &ReleaseProfileRepository,
        project_id: &str,
        profile_key: &str,
        binding: &ReleaseKeyProjectBinding,
        file_path: &Path,
        passphrase: &str,
    ) -> FormalResult<u32> {
        let _guard = RELEASE_KEY_OPERATION_LOCK.lock().await;
        let metadata = repository
            .credential_metadata(profile_key)
            .await?
            .ok_or_else(|| FormalError::NotFound(format!("发布配置不存在：{profile_key}")))?;
        if !metadata.is_project_key() {
            return Err(FormalError::Conflict(
                "当前发布凭据尚未迁移到项目主密钥，不能导入密钥包".into(),
            ));
        }
        let package = read_transfer_package(file_path)?;
        let expected_binding = binding.fingerprint()?;
        if package.project_binding != expected_binding {
            return Err(FormalError::Conflict(
                "项目主密钥包与当前平台、数据库或工作台Schema不匹配".into(),
            ));
        }
        if package.key_version != metadata.key_version {
            return Err(FormalError::Conflict(format!(
                "密钥包版本v{}与当前发布凭据版本v{}不一致",
                package.key_version, metadata.key_version
            )));
        }
        let key = decrypt_transfer_package(&package, passphrase)?;
        repository
            .get(&key, profile_key)
            .await
            .map_err(|_| FormalError::Conflict("密钥包无法解密当前发布凭据，导入已阻止".into()))?;
        self.save_key_tracked(project_id, &key).await?;
        Ok(key.version())
    }

    pub fn load_key(&self, project_id: &str, version: u32) -> FormalResult<ProjectMasterKey> {
        let reference = key_reference(project_id, version)?;
        let bytes = self.secrets.load(&reference).map_err(|error| match error {
            FormalError::NotFound(_) => {
                FormalError::Conflict("本机缺少当前项目主密钥，请从可信工作台导入加密密钥包".into())
            }
            other => other,
        })?;
        ProjectMasterKey::from_bytes(version, bytes)
    }

    pub fn save_key(&self, project_id: &str, key: &ProjectMasterKey) -> FormalResult<()> {
        let reference = key_reference(project_id, key.version())?;
        self.secrets.save(&reference, key.material())
    }

    async fn ensure_key(
        &self,
        project_id: &str,
        version: u32,
    ) -> FormalResult<(ProjectMasterKey, bool)> {
        let reference = key_reference(project_id, version)?;
        match self.secrets.load(&reference) {
            Ok(bytes) => {
                let key = ProjectMasterKey::from_bytes(version, bytes)?;
                self.track_key(project_id, version, &reference).await?;
                Ok((key, false))
            }
            Err(FormalError::NotFound(_)) => {
                let key = ProjectMasterKey::generate(version)?;
                self.save_key_tracked(project_id, &key).await?;
                Ok((key, true))
            }
            Err(error) => Err(error),
        }
    }

    async fn resolve_key_locked(
        &self,
        project_id: &str,
        metadata: CredentialMetadata,
    ) -> FormalResult<ProjectMasterKey> {
        if !metadata.is_project_key() {
            return Err(FormalError::InvalidConfig(
                "发布凭据来自不再支持的旧开发格式，请重建发布参数".into(),
            ));
        }
        let key = self.load_key(project_id, metadata.key_version)?;
        self.track_key(
            project_id,
            metadata.key_version,
            &key_reference(project_id, metadata.key_version)?,
        )
        .await?;
        Ok(key)
    }

    async fn save_key_tracked(&self, project_id: &str, key: &ProjectMasterKey) -> FormalResult<()> {
        let reference = key_reference(project_id, key.version())?;
        self.secrets.save(&reference, key.material())?;
        self.track_key(project_id, key.version(), &reference).await
    }

    async fn track_key(&self, project_id: &str, version: u32, reference: &str) -> FormalResult<()> {
        let Some(pool) = &self.local_pool else {
            return Ok(());
        };
        sqlx::query(
            "INSERT INTO local_project_master_key \
             (local_project_id, key_version, secret_ref, created_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(local_project_id, key_version) DO UPDATE SET secret_ref = excluded.secret_ref",
        )
        .bind(project_id)
        .bind(i64::from(version))
        .bind(reference)
        .bind(OffsetDateTime::now_utc().unix_timestamp_nanos().to_string())
        .execute(pool)
        .await
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "track project master key failed");
            FormalError::LocalDatabase("登记项目主密钥引用")
        })?;
        Ok(())
    }

    async fn delete_key_tracked(&self, project_id: &str, version: u32) -> FormalResult<()> {
        let reference = key_reference(project_id, version)?;
        if let Err(error) = self.secrets.delete(&reference) {
            if let Some(pool) = &self.local_pool {
                LocalProjectRepository::new(pool.clone(), self.secrets.clone())
                    .delete_secret_or_enqueue(&reference, "release_master_key_retired")
                    .await?;
                return Ok(());
            }
            return Err(error);
        }
        if let Some(pool) = &self.local_pool {
            sqlx::query(
                "DELETE FROM local_project_master_key \
                 WHERE local_project_id = ? AND key_version = ?",
            )
            .bind(project_id)
            .bind(i64::from(version))
            .execute(pool)
            .await
            .map_err(|error| {
                tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "untrack project master key failed");
                FormalError::LocalDatabase("清理项目主密钥引用")
            })?;
        }
        Ok(())
    }
}

fn validate_passphrase(passphrase: &str) -> FormalResult<()> {
    if !(12..=1024).contains(&passphrase.len()) {
        return Err(FormalError::InvalidConfig(
            "密钥包口令长度必须为12到1024个字节".into(),
        ));
    }
    Ok(())
}

fn transfer_aad(project_binding: &str, key_version: u32) -> String {
    format!("{TRANSFER_AAD_PREFIX}:{project_binding}:key-version:{key_version}")
}

fn encrypt_transfer_package(
    key: &ProjectMasterKey,
    project_binding: &str,
    passphrase: &str,
) -> FormalResult<EncryptedKeyPackage> {
    validate_passphrase(passphrase)?;
    let mut salt = [0_u8; 16];
    let mut nonce = [0_u8; 12];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let mut wrapping_key = derive_transfer_key(passphrase, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&wrapping_key).map_err(|_| {
        wrapping_key.zeroize();
        FormalError::InvalidConfig("初始化密钥包加密器失败".into())
    })?;
    let nonce_value = Nonce::try_from(nonce.as_slice())
        .map_err(|_| FormalError::InvalidConfig("密钥包随机数长度无效".into()))?;
    let aad = transfer_aad(project_binding, key.version());
    let mut plaintext = Zeroizing::new(key.material().to_vec());
    let ciphertext = cipher
        .encrypt(
            &nonce_value,
            aes_gcm::aead::Payload {
                msg: plaintext.as_slice(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| FormalError::InvalidConfig("加密项目主密钥包失败".into()))?;
    plaintext.zeroize();
    wrapping_key.zeroize();
    Ok(EncryptedKeyPackage {
        format: TRANSFER_FORMAT.into(),
        project_binding: project_binding.into(),
        key_version: key.version(),
        kdf: TRANSFER_KDF.into(),
        cipher: TRANSFER_CIPHER.into(),
        salt: STANDARD.encode(salt),
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    })
}

fn decrypt_transfer_package(
    package: &EncryptedKeyPackage,
    passphrase: &str,
) -> FormalResult<ProjectMasterKey> {
    validate_passphrase(passphrase)?;
    if package.format != TRANSFER_FORMAT
        || package.kdf != TRANSFER_KDF
        || package.cipher != TRANSFER_CIPHER
        || package.key_version == 0
    {
        return Err(FormalError::InvalidConfig(
            "不支持的项目主密钥包格式".into(),
        ));
    }
    let salt = decode_fixed::<16>(&package.salt, "密钥包盐值")?;
    let nonce = decode_fixed::<12>(&package.nonce, "密钥包随机数")?;
    let ciphertext = STANDARD
        .decode(&package.ciphertext)
        .map_err(|_| FormalError::InvalidConfig("密钥包密文编码无效".into()))?;
    if ciphertext.len() != super::credential_crypto::PROJECT_MASTER_KEY_BYTES + 16 {
        return Err(FormalError::InvalidConfig("密钥包密文长度无效".into()));
    }
    let mut wrapping_key = derive_transfer_key(passphrase, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&wrapping_key).map_err(|_| {
        wrapping_key.zeroize();
        FormalError::InvalidConfig("初始化密钥包解密器失败".into())
    })?;
    let nonce_value = Nonce::try_from(nonce.as_slice())
        .map_err(|_| FormalError::InvalidConfig("密钥包随机数长度无效".into()))?;
    let aad = transfer_aad(&package.project_binding, package.key_version);
    let plaintext = cipher
        .decrypt(
            &nonce_value,
            aes_gcm::aead::Payload {
                msg: &ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| FormalError::InvalidConfig("密钥包口令错误或内容已被篡改".into()))?;
    wrapping_key.zeroize();
    ProjectMasterKey::from_bytes(package.key_version, plaintext)
}

fn derive_transfer_key(passphrase: &str, salt: &[u8; 16]) -> FormalResult<[u8; 32]> {
    let mut key = [0_u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "derive transfer package key failed");
            FormalError::InvalidConfig("派生密钥包加密密钥失败".into())
        })?;
    Ok(key)
}

fn decode_fixed<const N: usize>(value: &str, label: &str) -> FormalResult<[u8; N]> {
    STANDARD
        .decode(value)
        .map_err(|_| FormalError::InvalidConfig(format!("{label}编码无效")))?
        .try_into()
        .map_err(|_| FormalError::InvalidConfig(format!("{label}长度无效")))
}

fn write_new_file_atomically(path: &Path, contents: &[u8]) -> FormalResult<()> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(FormalError::InvalidConfig(
            "项目主密钥包必须保存到绝对文件路径".into(),
        ));
    }
    if path.exists() {
        return Err(FormalError::Conflict(
            "目标密钥包文件已存在，请选择新的文件名".into(),
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| parent.is_dir())
        .ok_or(FormalError::LocalIo("密钥包目标目录不存在"))?;
    let temporary_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("release-master-key"),
        uuid::Uuid::now_v7()
    ));
    let mut temporary = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)
        .map_err(|error| {
            tracing::error!(error_kind = ?error.kind(), "create transfer package staging failed");
            FormalError::LocalIo("创建密钥包暂存文件")
        })?;
    temporary.write_all(contents).map_err(|error| {
        tracing::error!(error_kind = ?error.kind(), "write transfer package staging failed");
        FormalError::LocalIo("写入密钥包暂存文件")
    })?;
    temporary.sync_all().map_err(|error| {
        tracing::error!(error_kind = ?error.kind(), "sync transfer package staging failed");
        FormalError::LocalIo("同步密钥包暂存文件")
    })?;
    drop(temporary);
    std::fs::rename(&temporary_path, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary_path);
        tracing::error!(error_kind = ?error.kind(), "persist transfer package failed");
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            FormalError::Conflict("目标密钥包文件已存在，请选择新的文件名".into())
        } else {
            FormalError::LocalIo("提交密钥包文件")
        }
    })?;
    Ok(())
}

fn read_transfer_package(path: &Path) -> FormalResult<EncryptedKeyPackage> {
    if !path.is_absolute() || !path.is_file() {
        return Err(FormalError::InvalidConfig(
            "项目主密钥包文件不存在或路径无效".into(),
        ));
    }
    let metadata = std::fs::metadata(path).map_err(|error| {
        tracing::error!(error_kind = ?error.kind(), "read transfer package metadata failed");
        FormalError::LocalIo("读取密钥包元数据")
    })?;
    if metadata.len() == 0 || metadata.len() > MAX_TRANSFER_FILE_BYTES {
        return Err(FormalError::InvalidConfig("密钥包文件大小无效".into()));
    }
    let file = std::fs::File::open(path).map_err(|error| {
        tracing::error!(error_kind = ?error.kind(), "open transfer package failed");
        FormalError::LocalIo("打开项目主密钥包")
    })?;
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_TRANSFER_FILE_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|error| {
            tracing::error!(error_kind = ?error.kind(), "read transfer package failed");
            FormalError::LocalIo("读取项目主密钥包")
        })?;
    serde_json::from_slice(&contents)
        .map_err(|_| FormalError::InvalidConfig("项目主密钥包JSON无效".into()))
}

fn key_reference(project_id: &str, version: u32) -> FormalResult<String> {
    let project_id = project_id.trim();
    if project_id.is_empty()
        || project_id.len() > 128
        || !project_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || version == 0
    {
        return Err(FormalError::InvalidConfig("项目主密钥引用参数无效".into()));
    }
    Ok(format!("project/{project_id}/release-master-key/{version}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        ReleaseKeyProjectBinding, ReleaseMasterKeyManager, decrypt_transfer_package,
        encrypt_transfer_package, key_reference,
    };
    use crate::formal::credential_crypto::ProjectMasterKey;
    use crate::formal::local_store::LocalStore;
    use crate::formal::project_repository::{CreateLocalProject, LocalProjectRepository};
    use crate::formal::secret_store::MemorySecretStore;
    use crate::formal::secret_store::SecretStore;

    #[test]
    fn project_key_reference_is_scoped_and_material_is_not_exposed() {
        assert_eq!(
            key_reference("project-1", 2).expect("reference"),
            "project/project-1/release-master-key/2"
        );
        assert!(key_reference("../project", 1).is_err());
        let manager = ReleaseMasterKeyManager::new(Arc::new(MemorySecretStore::default()));
        let key = ProjectMasterKey::generate(1).expect("key");
        manager.save_key("project-1", &key).expect("save key");
        let loaded = manager.load_key("project-1", 1).expect("load key");
        assert_eq!(loaded.material(), key.material());
    }

    #[test]
    fn transfer_package_is_bound_to_project_version_and_passphrase() {
        let binding = ReleaseKeyProjectBinding {
            platform_url: "HTTPS://PLATFORM.EXAMPLE/".into(),
            db_host: "DB.EXAMPLE".into(),
            db_port: 3306,
            workbench_db: "INXAIOT_DESK_BUDDY".into(),
        };
        let fingerprint = binding.fingerprint().expect("binding");
        let key = ProjectMasterKey::generate(7).expect("key");
        let package = encrypt_transfer_package(&key, &fingerprint, "strong-passphrase")
            .expect("encrypt package");
        let loaded =
            decrypt_transfer_package(&package, "strong-passphrase").expect("decrypt package");
        assert_eq!(loaded.version(), 7);
        assert_eq!(loaded.material(), key.material());
        assert!(decrypt_transfer_package(&package, "wrong-passphrase").is_err());

        let mut tampered = package;
        tampered.project_binding = "different-project".into();
        assert!(decrypt_transfer_package(&tampered, "strong-passphrase").is_err());
    }

    #[test]
    fn transfer_binding_is_canonical_but_endpoint_sensitive() {
        let first = ReleaseKeyProjectBinding {
            platform_url: "https://Platform.Example/".into(),
            db_host: "DB.Example".into(),
            db_port: 3306,
            workbench_db: "Desk".into(),
        };
        let canonical_equivalent = ReleaseKeyProjectBinding {
            platform_url: "https://platform.example".into(),
            db_host: "db.example".into(),
            db_port: 3306,
            workbench_db: "desk".into(),
        };
        let other_schema = ReleaseKeyProjectBinding {
            workbench_db: "desk_other".into(),
            ..canonical_equivalent.clone()
        };
        assert_eq!(
            first.fingerprint().expect("first binding"),
            canonical_equivalent
                .fingerprint()
                .expect("equivalent binding")
        );
        assert_ne!(
            first.fingerprint().expect("first binding"),
            other_schema.fingerprint().expect("other binding")
        );
    }

    #[tokio::test]
    async fn tracked_master_key_is_removed_with_the_local_project() {
        let temporary = tempfile::tempdir().expect("temporary data directory");
        let local_store = LocalStore::open(temporary.path().join("local.db"))
            .await
            .expect("local store");
        let secrets = Arc::new(MemorySecretStore::default());
        let projects = LocalProjectRepository::new(local_store.pool().clone(), secrets.clone());
        let project = projects
            .create(CreateLocalProject {
                name: "tracked-key-project".into(),
                platform_url: "http://platform.test:8055".into(),
                db_host: "database.test".into(),
                db_port: 3306,
                db_user: "workbench".into(),
                db_password: "database-password".into(),
                business_db: "inxvision_iot_dev".into(),
                workbench_db: "inxaiot_desk_buddy".into(),
            })
            .await
            .expect("create project");
        let manager = ReleaseMasterKeyManager::with_local_registry(
            secrets.clone(),
            local_store.pool().clone(),
        );
        let key = ProjectMasterKey::generate(1).expect("master key");
        manager
            .save_key_tracked(&project.id, &key)
            .await
            .expect("save tracked master key");
        let reference = key_reference(&project.id, 1).expect("key reference");
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM local_project_master_key WHERE local_project_id = ?",
            )
            .bind(&project.id)
            .fetch_one(local_store.pool())
            .await
            .expect("tracked key count"),
            1
        );
        assert!(secrets.load(&reference).is_ok());

        projects.delete(&project.id).await.expect("delete project");
        assert!(secrets.load(&reference).is_err());
        assert_eq!(
            projects
                .pending_secret_cleanup_count()
                .await
                .expect("pending cleanup"),
            0
        );
        local_store.close().await;
    }
}
