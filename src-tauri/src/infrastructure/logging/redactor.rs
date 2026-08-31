use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

use crate::core::error::{AppError, AppResult};
use crate::domain::common::task::TaskEvent;

const REDACTED: &str = "[REDACTED]";
const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "secret",
    "token",
    "authkey",
    "privatekey",
    "authorization",
];

#[derive(Clone)]
pub struct SensitiveValueRedactor {
    secret_values: Arc<RwLock<Vec<String>>>,
}

impl std::fmt::Debug for SensitiveValueRedactor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let secret_count = self
            .secret_values
            .read()
            .map(|values| values.len())
            .unwrap_or_default();
        formatter
            .debug_struct("SensitiveValueRedactor")
            .field("secret_count", &secret_count)
            .finish()
    }
}

impl Default for SensitiveValueRedactor {
    fn default() -> Self {
        Self::new(std::iter::empty())
    }
}

impl SensitiveValueRedactor {
    pub fn new(values: impl IntoIterator<Item = String>) -> Self {
        let mut secret_values = values
            .into_iter()
            .filter(|value| value.len() >= 3)
            .collect::<Vec<_>>();
        secret_values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        secret_values.dedup();
        Self {
            secret_values: Arc::new(RwLock::new(secret_values)),
        }
    }

    pub fn production() -> Self {
        static PRODUCTION: OnceLock<SensitiveValueRedactor> = OnceLock::new();
        PRODUCTION.get_or_init(Self::default).clone()
    }

    pub fn register_values(&self, values: impl IntoIterator<Item = String>) -> AppResult<()> {
        let mut secret_values = self
            .secret_values
            .write()
            .map_err(|_| AppError::Conflict("敏感值脱敏注册表已损坏".into()))?;
        secret_values.extend(values.into_iter().filter(|value| value.len() >= 3));
        secret_values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        secret_values.dedup();
        Ok(())
    }

    pub fn with_additional_values(&self, values: impl IntoIterator<Item = String>) -> Self {
        let clone = self.clone();
        if clone.register_values(values).is_err() {
            tracing::error!("register sensitive values failed");
        }
        clone
    }

    pub fn redact_event(&self, event: &mut TaskEvent) {
        event.message_code = self.redact_text(&event.message_code);
        event.message = event.message.take().map(|value| self.redact_text(&value));
        event.message_params = self.redact_params(std::mem::take(&mut event.message_params));
    }

    pub fn redact_text(&self, value: &str) -> String {
        let Ok(secret_values) = self.secret_values.read() else {
            return REDACTED.into();
        };
        secret_values
            .iter()
            .fold(value.to_string(), |text, secret| {
                text.replace(secret, REDACTED)
            })
    }

    fn redact_params(&self, values: BTreeMap<String, String>) -> BTreeMap<String, String> {
        values
            .into_iter()
            .map(|(key, value)| {
                let value = if is_sensitive_key(&key) {
                    REDACTED.into()
                } else {
                    self.redact_text(&value)
                };
                (key, value)
            })
            .collect()
    }
}

fn is_sensitive_key(value: &str) -> bool {
    let canonical = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    SENSITIVE_KEYS
        .iter()
        .any(|sensitive| canonical.contains(sensitive))
}
