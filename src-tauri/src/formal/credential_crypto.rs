use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::Argon2;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use super::error::{FormalError, FormalResult};

const SCHEME: &str = "argon2id-aes256gcm";
const KEY_VERSION: u32 = 1;
const ASSOCIATED_DATA: &[u8] = b"inxaiot-desk-buddy:aio-release-profile:v1";

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

#[derive(Clone, Debug)]
pub struct CredentialEnvelope {
    pub scheme: String,
    pub key_version: u32,
    pub salt: [u8; 16],
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}

pub fn encrypt_release_credentials(
    database_password: &str,
    credentials: &ReleaseCredentials,
) -> FormalResult<CredentialEnvelope> {
    if database_password.is_empty() {
        return Err(FormalError::InvalidConfig("数据库密码不能为空".into()));
    }
    let mut salt = [0_u8; 16];
    let mut nonce = [0_u8; 12];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let mut key = derive_key(database_password, &salt)?;
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
                aad: ASSOCIATED_DATA,
            },
        )
        .map_err(|_| FormalError::InvalidConfig("加密发布凭据失败".into()))?;
    plaintext.zeroize();
    key.zeroize();
    Ok(CredentialEnvelope {
        scheme: SCHEME.into(),
        key_version: KEY_VERSION,
        salt,
        nonce,
        ciphertext,
    })
}

pub fn decrypt_release_credentials(
    database_password: &str,
    envelope: &CredentialEnvelope,
) -> FormalResult<ReleaseCredentials> {
    if envelope.scheme != SCHEME || envelope.key_version != KEY_VERSION {
        return Err(FormalError::InvalidConfig("不支持的凭据加密格式".into()));
    }
    let mut key = derive_key(database_password, &envelope.salt)?;
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
                aad: ASSOCIATED_DATA,
            },
        )
        .map_err(|_| FormalError::InvalidConfig("发布凭据解密失败".into()))?;
    key.zeroize();
    let result = serde_json::from_slice(&plaintext)
        .map_err(|_| FormalError::InvalidConfig("解析发布凭据失败".into()));
    plaintext.zeroize();
    result
}

fn derive_key(database_password: &str, salt: &[u8; 16]) -> FormalResult<[u8; 32]> {
    let mut key = [0_u8; 32];
    Argon2::default()
        .hash_password_into(database_password.as_bytes(), salt, &mut key)
        .map_err(|error| {
            tracing::error!(error = ?error, "derive release credential key failed");
            FormalError::InvalidConfig("派生发布凭据密钥失败".into())
        })?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::{ReleaseCredentials, decrypt_release_credentials, encrypt_release_credentials};

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
    fn credential_round_trip_uses_authenticated_encryption() {
        let credentials = fixture();
        let envelope = encrypt_release_credentials("database-password", &credentials)
            .expect("encrypt credentials");
        let cipher_text = String::from_utf8_lossy(&envelope.ciphertext);
        assert!(!cipher_text.contains("auth-key-secret"));
        assert!(!cipher_text.contains("private-key-secret"));
        assert_eq!(
            decrypt_release_credentials("database-password", &envelope)
                .expect("decrypt credentials"),
            credentials
        );
        assert!(decrypt_release_credentials("wrong-password", &envelope).is_err());
        assert!(!format!("{credentials:?}").contains("auth-key-secret"));
    }
}
