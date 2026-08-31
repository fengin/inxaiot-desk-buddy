use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rand::rngs::OsRng;
use reqwest::Client;
use rsa::pkcs1::{DecodeRsaPublicKey, DecodeRsaPublicKey as _};
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use serde_json::{Value, json};

use crate::core::error::{AppError, AppResult};
use crate::core::secret::SecretValue;

#[derive(Clone, Debug)]
pub struct PlatformLoginConfig {
    pub base_url: String,
    pub principal: String,
    pub credentials: SecretValue,
    pub session_uuid: SecretValue,
    pub image_code: SecretValue,
    pub timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct PlatformSession {
    pub principal: String,
    pub token_type: String,
    pub access_token: SecretValue,
}

pub struct PlatformAuthClient {
    client: Client,
}

impl PlatformAuthClient {
    pub fn new(timeout: Duration) -> AppResult<Self> {
        let client = Client::builder()
            .timeout(timeout)
            .cookie_store(true)
            .user_agent(concat!("inxaiot-desk-buddy/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| AppError::platform_http("创建HTTP客户端", &error))?;
        Ok(Self { client })
    }

    pub async fn login(&self, config: &PlatformLoginConfig) -> AppResult<PlatformSession> {
        let base_url = normalize_base_url(&config.base_url);
        let envelope = self
            .client
            .get(format!("{base_url}/rsa/public/key"))
            .send()
            .await
            .map_err(|error| AppError::platform_http("获取平台RSA公钥", &error))?
            .error_for_status()
            .map_err(|error| AppError::platform_http("获取平台RSA公钥", &error))?
            .json::<Value>()
            .await
            .map_err(|error| AppError::platform_http("解析平台RSA公钥", &error))?;
        ensure_success(&envelope, "获取平台RSA公钥")?;
        let data = envelope
            .get("data")
            .ok_or_else(|| AppError::Authentication("平台RSA公钥响应缺少data".into()))?;
        let public_key = data["publicKey"]
            .as_str()
            .ok_or_else(|| AppError::Authentication("平台RSA公钥响应缺少publicKey".into()))?;
        let key = data["key"]
            .as_str()
            .ok_or_else(|| AppError::Authentication("平台RSA公钥响应缺少key".into()))?;
        let encrypted = encrypt_password(public_key, config.credentials.expose())?;

        let envelope = self
            .client
            .post(format!(
                "{base_url}/login?grant_type=edge&client_id=0&key={key}"
            ))
            .json(&json!({
                "principal": config.principal,
                "credentials": encrypted,
                "sessionUUID": config.session_uuid.expose(),
                "imageCode": config.image_code.expose(),
                "key": key
            }))
            .send()
            .await
            .map_err(|error| AppError::platform_http("提交平台登录", &error))?
            .error_for_status()
            .map_err(|error| AppError::platform_http("提交平台登录", &error))?
            .json::<Value>()
            .await
            .map_err(|error| AppError::platform_http("解析平台登录响应", &error))?;
        ensure_success(&envelope, "平台登录")?;
        let data = envelope
            .get("data")
            .ok_or_else(|| AppError::Authentication("平台登录响应缺少data".into()))?;
        let access_token = data["access_token"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::Authentication("平台登录成功但未返回访问令牌".into()))?;
        Ok(PlatformSession {
            principal: config.principal.clone(),
            token_type: data["token_type"].as_str().unwrap_or("Bearer").to_string(),
            access_token: SecretValue::new(access_token),
        })
    }
}

fn normalize_base_url(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        format!("http://{value}")
    }
}

fn ensure_success(envelope: &Value, operation: &str) -> AppResult<()> {
    if envelope
        .get("code")
        .is_some_and(|code| code.as_i64() == Some(200) || code.as_str() == Some("200"))
    {
        return Ok(());
    }
    let message = envelope
        .get("message")
        .or_else(|| envelope.get("msg"))
        .and_then(Value::as_str)
        .unwrap_or("平台未返回明确原因");
    Err(AppError::Authentication(format!("{operation}失败：{message}")))
}

fn encrypt_password(public_key: &str, password: &str) -> AppResult<String> {
    let parsed = if public_key.contains("BEGIN") {
        RsaPublicKey::from_public_key_pem(public_key)
            .or_else(|_| RsaPublicKey::from_pkcs1_pem(public_key))
    } else {
        let der = STANDARD.decode(public_key).map_err(|error| {
            tracing::error!(error = ?error, "failed to decode platform rsa public key");
            AppError::Authentication("无法解码平台RSA公钥".into())
        })?;
        RsaPublicKey::from_public_key_der(&der).or_else(|_| RsaPublicKey::from_pkcs1_der(&der))
    }
    .map_err(|error| {
        tracing::error!(error = ?error, "failed to parse platform rsa public key");
        AppError::Authentication("无法解析平台RSA公钥".into())
    })?;
    let encrypted = parsed
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, password.as_bytes())
        .map_err(|error| {
            tracing::error!(error = ?error, "failed to encrypt platform password");
            AppError::Authentication("平台密码加密失败".into())
        })?;
    Ok(STANDARD.encode(encrypted))
}

