use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::core::error::{AppError, AppResult};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AioRegistrationPayload {
    pub name: String,
    pub ip: String,
    pub mac: String,
    pub platform_ip: String,
    pub platform_port: String,
    pub auth_key: String,
    pub building_id: Option<i64>,
    pub addr_alias: Option<String>,
}

#[derive(Clone)]
pub struct DeviceApiClient {
    client: Client,
    base_url: String,
    auth_key: String,
}

impl DeviceApiClient {
    pub fn new(base_url: &str, auth_key: &str) -> AppResult<Self> {
        let base_url = base_url.trim().trim_end_matches('/').to_string();
        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            return Err(AppError::InvalidConfig("一体机API地址无效".into()));
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| AppError::InvalidConfig("无法创建一体机API客户端".into()))?;
        Ok(Self {
            client,
            base_url,
            auth_key: auth_key.trim().into(),
        })
    }

    pub async fn exists(&self) -> AppResult<bool> {
        let request = self
            .client
            .get(format!("{}/api/aio/server/exist", self.base_url));
        let envelope = self.send(request).await?;
        envelope
            .data
            .as_bool()
            .ok_or_else(|| AppError::InvalidConfig("一体机exist响应无效".into()))
    }

    pub async fn register_if_missing(&self, payload: &AioRegistrationPayload) -> AppResult<()> {
        validate_payload(payload)?;
        if self.exists().await? {
            return Ok(());
        }
        let request = self
            .client
            .post(format!("{}/api/aio/server/save", self.base_url))
            .json(payload);
        let envelope = self.send(request).await?;
        if !success_code(envelope.code) {
            return Err(AppError::PlatformHttp {
                operation: "一体机本地注册",
            });
        }
        Ok(())
    }

    pub async fn register_if_missing_with_retry(
        &self,
        payload: &AioRegistrationPayload,
        attempts: u32,
        interval: Duration,
    ) -> AppResult<()> {
        if attempts == 0 || interval.is_zero() {
            return Err(AppError::InvalidConfig("注册重试参数无效".into()));
        }
        let mut last_error = None;
        for attempt in 0..attempts {
            match self.register_if_missing(payload).await {
                Ok(()) => return Ok(()),
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < attempts {
                tokio::time::sleep(interval).await;
            }
        }
        Err(last_error.unwrap_or(AppError::PlatformHttp {
            operation: "一体机本地注册重试",
        }))
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> AppResult<ApiEnvelope> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AppError::InvalidConfig("系统时间无效".into()))?
            .as_millis()
            .to_string();
        let signature = format!(
            "{:x}",
            md5::compute(format!("{}{}", self.auth_key, timestamp))
        );
        let response = request
            .header("aioAuthorization", signature)
            .header("ts", timestamp)
            .send()
            .await
            .map_err(|error| AppError::platform_http("调用一体机本地API", &error))?
            .error_for_status()
            .map_err(|error| AppError::platform_http("一体机本地API状态", &error))?
            .json::<ApiEnvelope>()
            .await
            .map_err(|error| AppError::platform_http("解析一体机本地API", &error))?;
        if !success_code(response.code) {
            tracing::warn!(code = response.code, message = %response.message, "device api rejected request");
        }
        Ok(response)
    }
}

#[derive(Deserialize)]
struct ApiEnvelope {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: serde_json::Value,
}

fn success_code(code: i64) -> bool {
    code == 0 || code == 200
}

fn validate_payload(payload: &AioRegistrationPayload) -> AppResult<()> {
    if payload.name.trim().is_empty()
        || payload.ip.trim().is_empty()
        || payload.mac.trim().is_empty()
        || payload.platform_ip.trim().is_empty()
        || payload.platform_port.trim().is_empty()
        || payload.auth_key.trim().is_empty()
    {
        return Err(AppError::InvalidConfig("一体机注册参数不完整".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AioRegistrationPayload, validate_payload};

    #[test]
    fn registration_requires_stable_identity_and_platform_parameters() {
        let valid = AioRegistrationPayload {
            name: "aio".into(),
            ip: "192.0.2.1".into(),
            mac: "00:11:22:33:44:55".into(),
            platform_ip: "10.0.0.1".into(),
            platform_port: "8055".into(),
            auth_key: "key".into(),
            building_id: None,
            addr_alias: None,
        };
        assert!(validate_payload(&valid).is_ok());
        let mut invalid = valid;
        invalid.auth_key.clear();
        assert!(validate_payload(&invalid).is_err());
    }
}
