use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::Argon2;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::error::{FormalError, FormalResult};

pub const PROJECT_KEY_CREDENTIAL_SCHEME: &str = "argon2id-aes256gcm-project-key";
pub const PROJECT_MASTER_KEY_BYTES: usize = 32;
const PROJECT_KEY_ASSOCIATED_DATA_PREFIX: &str =
    "inxaiot-desk-buddy:aio-release-profile:project-key:v1";

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseCredentials {
    pub platform_auth_key: String,
    pub platform_mqtt_user: String,
    pub platform_mqtt_password: String,
    pub aio_mqtt_user: String,
    pub aio_mqtt_password: String,
    pub ssh_user: String,
    pub ssh_password: Option<String>,
    pub ssh_private_key: Option<String>,
}

impl std::fmt::Debug for ReleaseCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ReleaseCredentials([REDACTED])")
    }
}

#[derive(Clone)]
pub struct ProjectMasterKey {
    version: u32,
    material: Zeroizing<Vec<u8>>,
}

impl ProjectMasterKey {
    pub fn generate(version: u32) -> FormalResult<Self> {
        if version == 0 {
            return Err(FormalError::InvalidConfig("项目主密钥版本必须大于0".into()));
        }
        let mut material = vec![0_u8; PROJECT_MASTER_KEY_BYTES];
        OsRng.fill_bytes(&mut material);
        Ok(Self {
            version,
            material: Zeroizing::new(material),
        })
    }

    pub fn from_bytes(version: u32, material: Vec<u8>) -> FormalResult<Self> {
        if version == 0 || material.len() != PROJECT_MASTER_KEY_BYTES {
            return Err(FormalError::InvalidConfig(
                "项目主密钥版本或长度无效".into(),
            ));
        }
        Ok(Self {
            version,
            material: Zeroizing::new(material),
        })
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn material(&self) -> &[u8] {
        self.material.as_slice()
    }
}

impl std::fmt::Debug for ProjectMasterKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectMasterKey")
            .field("version", &self.version)
            .field("material", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct CredentialEnvelope {
    pub scheme: String,
    pub key_version: u32,
    pub salt: [u8; 16],
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialMetadata {
    pub scheme: String,
    pub key_version: u32,
}

impl CredentialMetadata {
    pub fn is_project_key(&self) -> bool {
        self.scheme == PROJECT_KEY_CREDENTIAL_SCHEME && self.key_version > 0
    }
}

pub fn encrypt_release_credentials(
    key: &ProjectMasterKey,
    credentials: &ReleaseCredentials,
) -> FormalResult<CredentialEnvelope> {
    let associated_data = project_key_associated_data(key.version());
    encrypt_with_material(
        key.material(),
        PROJECT_KEY_CREDENTIAL_SCHEME,
        key.version(),
        associated_data.as_bytes(),
        credentials,
    )
}

pub fn decrypt_release_credentials(
    key: &ProjectMasterKey,
    envelope: &CredentialEnvelope,
) -> FormalResult<ReleaseCredentials> {
    if envelope.scheme != PROJECT_KEY_CREDENTIAL_SCHEME
        || envelope.key_version == 0
        || envelope.key_version != key.version()
    {
        return Err(FormalError::InvalidConfig(
            "发布凭据与项目主密钥版本不匹配".into(),
        ));
    }
    let associated_data = project_key_associated_data(envelope.key_version);
    decrypt_with_material(key.material(), envelope, associated_data.as_bytes())
}

fn encrypt_with_material(
    material: &[u8],
    scheme: &str,
    key_version: u32,
    associated_data: &[u8],
    credentials: &ReleaseCredentials,
) -> FormalResult<CredentialEnvelope> {
    if material.is_empty() {
        return Err(FormalError::InvalidConfig("凭据加密密钥不能为空".into()));
    }
    let mut salt = [0_u8; 16];
    let mut nonce = [0_u8; 12];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let mut key = derive_key(material, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| {
        key.zeroize();
        FormalError::InvalidConfig("初始化凭据加密器失败".into())
    })?;
    let mut plaintext = serde_json::to_vec(credentials)
        .map_err(|_| FormalError::InvalidConfig("序列化发布凭据失败".into()))?;
    let nonce_value = Nonce::try_from(nonce.as_slice())
        .map_err(|_| FormalError::InvalidConfig("凭据加密随机数长度无效".into()))?;
    let ciphertext = cipher
        .encrypt(
            &nonce_value,
            aes_gcm::aead::Payload {
                msg: &plaintext,
                aad: associated_data,
            },
        )
        .map_err(|_| FormalError::InvalidConfig("加密发布凭据失败".into()))?;
    plaintext.zeroize();
    key.zeroize();
    Ok(CredentialEnvelope {
        scheme: scheme.into(),
        key_version,
        salt,
        nonce,
        ciphertext,
    })
}

fn decrypt_with_material(
    material: &[u8],
    envelope: &CredentialEnvelope,
    associated_data: &[u8],
) -> FormalResult<ReleaseCredentials> {
    if material.is_empty() {
        return Err(FormalError::InvalidConfig("凭据解密密钥不能为空".into()));
    }
    let mut key = derive_key(material, &envelope.salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| {
        key.zeroize();
        FormalError::InvalidConfig("初始化凭据解密器失败".into())
    })?;
    let nonce_value = Nonce::try_from(envelope.nonce.as_slice())
        .map_err(|_| FormalError::InvalidConfig("凭据解密随机数长度无效".into()))?;
    let mut plaintext = cipher
        .decrypt(
            &nonce_value,
            aes_gcm::aead::Payload {
                msg: &envelope.ciphertext,
                aad: associated_data,
            },
        )
        .map_err(|_| FormalError::InvalidConfig("发布凭据解密失败".into()))?;
    key.zeroize();
    let result = serde_json::from_slice(&plaintext)
        .map_err(|_| FormalError::InvalidConfig("解析发布凭据失败".into()));
    plaintext.zeroize();
    result
}

fn derive_key(material: &[u8], salt: &[u8; 16]) -> FormalResult<[u8; 32]> {
    let mut key = [0_u8; 32];
    Argon2::default()
        .hash_password_into(material, salt, &mut key)
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "derive release credential key failed");
            FormalError::InvalidConfig("派生发布凭据密钥失败".into())
        })?;
    Ok(key)
}

fn project_key_associated_data(version: u32) -> String {
    format!("{PROJECT_KEY_ASSOCIATED_DATA_PREFIX}:key-version:{version}")
}

#[cfg(test)]
mod tests {
    use super::{
        PROJECT_KEY_CREDENTIAL_SCHEME, ProjectMasterKey, ReleaseCredentials,
        decrypt_release_credentials, encrypt_release_credentials,
    };

    fn fixture() -> ReleaseCredentials {
        ReleaseCredentials {
            platform_auth_key: "auth-key-secret".into(),
            platform_mqtt_user: "platform-user".into(),
            platform_mqtt_password: "platform-password".into(),
            aio_mqtt_user: "aio-user".into(),
            aio_mqtt_password: "aio-password".into(),
            ssh_user: "root".into(),
            ssh_password: None,
            ssh_private_key: Some("private-key-secret".into()),
        }
    }

    #[test]
    fn project_master_key_round_trip_is_version_bound() {
        let credentials = fixture();
        let key = ProjectMasterKey::generate(3).expect("generate project key");
        let envelope =
            encrypt_release_credentials(&key, &credentials).expect("encrypt credentials");
        let cipher_text = String::from_utf8_lossy(&envelope.ciphertext);
        assert_eq!(envelope.scheme, PROJECT_KEY_CREDENTIAL_SCHEME);
        assert_eq!(envelope.key_version, 3);
        assert!(!cipher_text.contains("auth-key-secret"));
        assert!(!cipher_text.contains("private-key-secret"));
        assert_eq!(
            decrypt_release_credentials(&key, &envelope).expect("decrypt credentials"),
            credentials
        );
        let wrong_key = ProjectMasterKey::generate(3).expect("generate wrong key");
        assert!(decrypt_release_credentials(&wrong_key, &envelope).is_err());
        let wrong_version =
            ProjectMasterKey::from_bytes(4, key.material().to_vec()).expect("wrong version key");
        assert!(decrypt_release_credentials(&wrong_version, &envelope).is_err());
        assert!(!format!("{key:?}").contains(&hex::encode(key.material())));
    }
}
