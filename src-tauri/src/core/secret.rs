use std::fmt;

use zeroize::Zeroize;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::SecretValue;

    #[test]
    fn debug_output_never_contains_secret() {
        let value = SecretValue::new("do-not-print");
        let output = format!("{value:?}");
        assert!(!output.contains("do-not-print"));
        assert!(output.contains("REDACTED"));
    }
}
