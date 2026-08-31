include!("operation_repository.rs");

const STRICT_FORBIDDEN_KEYS: &[&str] = &[
    "password",
    "privatekey",
    "authkey",
    "token",
    "localpath",
    "progress",
    "steps",
    "stdout",
    "stderr",
    "log",
];

pub fn validate_summary_json_strict(value: &serde_json::Value) -> FormalResult<()> {
    fn canonical_key(value: &str) -> String {
        value
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    }

    fn contains_forbidden(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(map) => map.iter().any(|(key, value)| {
                let key = canonical_key(key);
                STRICT_FORBIDDEN_KEYS
                    .iter()
                    .any(|forbidden| key.contains(forbidden))
                    || contains_forbidden(value)
            }),
            serde_json::Value::Array(values) => values.iter().any(contains_forbidden),
            _ => false,
        }
    }

    if contains_forbidden(value) {
        return Err(FormalError::InvalidConfig(
            "操作摘要包含过程数据或敏感字段".into(),
        ));
    }
    Ok(())
}

#[derive(Clone)]
pub struct StrictOperationRepository {
    inner: OperationRepository,
}

impl StrictOperationRepository {
    pub fn new(pool: sqlx::MySqlPool) -> Self {
        Self {
            inner: OperationRepository::new(pool),
        }
    }

    pub async fn start(&self, input: OperationStart) -> FormalResult<OperationRecord> {
        if let Some(summary) = &input.operation_summary {
            validate_summary_json_strict(summary)?;
        }
        self.inner.start(input).await
    }

    pub async fn heartbeat(&self, operation_id: &str, expected_version: u64) -> FormalResult<u64> {
        self.inner.heartbeat(operation_id, expected_version).await
    }

    pub async fn finalize_target(&self, result: TargetFinalResult) -> FormalResult<()> {
        self.inner.finalize_target(result).await
    }

    pub async fn finalize(&self, result: OperationFinalResult) -> FormalResult<OperationRecord> {
        self.inner.finalize(result).await
    }

    pub async fn get(&self, operation_id: &str) -> FormalResult<OperationRecord> {
        self.inner.get(operation_id).await
    }

    pub async fn delete_test_operation(&self, operation_id: &str) -> FormalResult<()> {
        self.inner.delete_test_operation(operation_id).await
    }
}

