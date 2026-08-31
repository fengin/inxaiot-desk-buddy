use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rand::rngs::OsRng;
use reqwest::{Client, StatusCode};
use rsa::pkcs1::DecodeRsaPublicKey;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use serde_json::{Value, json};
use time::{Duration as TimeDuration, OffsetDateTime};

use super::error::{FormalError, FormalResult};

const DEFAULT_SESSION_TTL: TimeDuration = TimeDuration::minutes(30);

#[derive(Clone)]
pub struct PlatformLoginSpec {
    pub base_url: String,
    pub principal: String,
    pub password: String,
    pub session_uuid: String,
    pub image_code: String,
    pub timeout: Duration,
}

impl std::fmt::Debug for PlatformLoginSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlatformLoginSpec")
            .field("base_url", &self.base_url)
            .field("principal", &self.principal)
            .field("password", &"[REDACTED]")
            .field("session_uuid", &"[REDACTED]")
            .field("image_code", &"[REDACTED]")
            .field("timeout", &self.timeout)
            .finish()
    }
}

pub struct PlatformSession {
    pub principal: String,
    pub token_type: String,
    pub access_token: String,
    pub expires_at: Option<OffsetDateTime>,
}

impl std::fmt::Debug for PlatformSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlatformSession")
            .field("principal", &self.principal)
            .field("token_type", &self.token_type)
            .field("access_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Clone)]
pub struct PlatformAuthAdapter {
    client: Client,
}

impl PlatformAuthAdapter {
    pub fn new(timeout: Duration) -> FormalResult<Self> {
        let client = Client::builder()
            .timeout(timeout)
            .cookie_store(true)
            .user_agent(concat!("inxaiot-desk-buddy/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "build platform http client failed");
                FormalError::InvalidConfig("无法创建平台HTTP客户端".into())
            })?;
        Ok(Self { client })
    }

    pub async fn login(&self, spec: &PlatformLoginSpec) -> FormalResult<PlatformSession> {
        validate_login_spec(spec)?;
        let base_url = normalize_base_url(&spec.base_url);
        let key_envelope = self
            .client
            .get(format!("{base_url}/rsa/public/key"))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| map_http_error("获取平台RSA公钥", error))?
            .json::<Value>()
            .await
            .map_err(|error| map_http_error("解析平台RSA公钥", error))?;
        ensure_success(&key_envelope, "获取平台RSA公钥")?;
        let data = key_envelope
            .get("data")
            .ok_or_else(|| FormalError::InvalidConfig("平台RSA响应缺少data".into()))?;
        let public_key = data["publicKey"]
            .as_str()
            .ok_or_else(|| FormalError::InvalidConfig("平台RSA响应缺少publicKey".into()))?;
        let request_key = data["key"]
            .as_str()
            .ok_or_else(|| FormalError::InvalidConfig("平台RSA响应缺少key".into()))?;
        let encrypted = encrypt_password(public_key, &spec.password)?;

        let login_envelope = self
            .client
            .post(format!(
                "{base_url}/login?grant_type=edge&client_id=0&key={request_key}"
            ))
            .json(&json!({
                "principal": spec.principal,
                "credentials": encrypted,
                "sessionUUID": spec.session_uuid,
                "imageCode": spec.image_code,
                "key": request_key
            }))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| map_http_error("提交平台登录", error))?
            .json::<Value>()
            .await
            .map_err(|error| map_http_error("解析平台登录响应", error))?;
        ensure_success(&login_envelope, "平台登录")?;
        let data = login_envelope
            .get("data")
            .ok_or_else(|| FormalError::InvalidConfig("平台登录响应缺少data".into()))?;
        let access_token = data["access_token"]
            .as_str()
            .filter(|value| value.len() >= 3)
            .ok_or_else(|| FormalError::InvalidConfig("平台登录未返回访问令牌".into()))?;
        let expires_at = data["expires_in"]
            .as_i64()
            .filter(|seconds| *seconds > 0)
            .map(|seconds| OffsetDateTime::now_utc() + TimeDuration::seconds(seconds))
            .unwrap_or_else(|| OffsetDateTime::now_utc() + DEFAULT_SESSION_TTL);
        Ok(PlatformSession {
            principal: spec.principal.clone(),
            token_type: data["token_type"].as_str().unwrap_or("Bearer").to_string(),
            access_token: access_token.to_string(),
            expires_at: Some(expires_at),
        })
    }

    pub async fn validate_access_token(
        &self,
        base_url: &str,
        access_token: &str,
    ) -> FormalResult<bool> {
        if base_url.trim().is_empty() || access_token.len() < 3 {
            return Err(FormalError::InvalidConfig("平台会话校验参数不完整".into()));
        }
        let response = self
            .client
            .get(format!("{}/sys/menu/nav", normalize_base_url(base_url)))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|error| map_http_error("校验平台会话", error))?;
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return Ok(false);
        }
        let envelope = response
            .error_for_status()
            .map_err(|error| map_http_error("校验平台会话", error))?
            .json::<Value>()
            .await
            .map_err(|error| map_http_error("解析平台会话校验响应", error))?;
        if let Some(code) = envelope.get("code") {
            return Ok(code.as_i64() == Some(200) || code.as_str() == Some("200"));
        }
        Ok(envelope.as_object().is_some_and(|object| {
            object.contains_key("menuList") && object.contains_key("authorities")
        }))
    }
}

fn validate_login_spec(spec: &PlatformLoginSpec) -> FormalResult<()> {
    if spec.base_url.trim().is_empty()
        || spec.principal.trim().is_empty()
        || spec.password.len() < 3
        || spec.session_uuid.is_empty()
        || spec.image_code.is_empty()
    {
        return Err(FormalError::InvalidConfig(
            "平台登录参数不完整，且密码至少需要3个字节".into(),
        ));
    }
    Ok(())
}

fn normalize_base_url(value: &str) -> String {
    let value = value.trim().trim_end_matches('/');
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_string()
    } else {
        format!("http://{value}")
    }
}

fn ensure_success(envelope: &Value, operation: &str) -> FormalResult<()> {
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
    Err(FormalError::InvalidConfig(format!(
        "{operation}失败：{message}"
    )))
}

fn encrypt_password(public_key: &str, password: &str) -> FormalResult<String> {
    let parsed = if public_key.contains("BEGIN") {
        RsaPublicKey::from_public_key_pem(public_key)
            .or_else(|_| RsaPublicKey::from_pkcs1_pem(public_key))
    } else {
        let der = STANDARD.decode(public_key).map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "decode platform rsa key failed");
            FormalError::InvalidConfig("无法解码平台RSA公钥".into())
        })?;
        RsaPublicKey::from_public_key_der(&der).or_else(|_| RsaPublicKey::from_pkcs1_der(&der))
    }
    .map_err(|error| {
        tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "parse platform rsa key failed");
        FormalError::InvalidConfig("无法解析平台RSA公钥".into())
    })?;
    let encrypted = parsed
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, password.as_bytes())
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "encrypt platform password failed");
            FormalError::InvalidConfig("平台密码加密失败".into())
        })?;
    Ok(STANDARD.encode(encrypted))
}

fn map_http_error(operation: &'static str, error: reqwest::Error) -> FormalError {
    let _ = error;
    tracing::error!(operation, "platform http operation failed");
    FormalError::InvalidConfig(format!("{operation}失败"))
}
