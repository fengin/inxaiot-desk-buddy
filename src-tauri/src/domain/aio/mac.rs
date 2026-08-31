use serde::{Deserialize, Serialize};

use crate::core::error::{AppError, AppResult};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MacAddress(String);

impl MacAddress {
    pub fn parse(value: &str) -> AppResult<Self> {
        let normalized = value
            .chars()
            .filter(|character| !matches!(character, ':' | '-' | ' ' | '.'))
            .flat_map(char::to_uppercase)
            .collect::<String>();
        if normalized.len() != 12
            || !normalized
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_lowercase())
        {
            return Err(AppError::InvalidConfig("MAC地址格式无效".into()));
        }
        Ok(Self(normalized))
    }

    pub fn normalized(&self) -> &str {
        &self.0
    }

    pub fn display(&self) -> String {
        self.0
            .as_bytes()
            .chunks(2)
            .map(|chunk| std::str::from_utf8(chunk).expect("validated ASCII MAC"))
            .collect::<Vec<_>>()
            .join(":")
    }
}

#[cfg(test)]
mod tests {
    use super::MacAddress;

    #[test]
    fn mac_identity_is_normalized_and_displayed_consistently() {
        for input in [
            "00:0c:29:3b:b9:31",
            "00-0C-29-3B-B9-31",
            "000c.293b.b931",
            "00 0c 29 3b b9 31",
        ] {
            let mac = MacAddress::parse(input).expect("valid MAC");
            assert_eq!(mac.normalized(), "000C293BB931");
            assert_eq!(mac.display(), "00:0C:29:3B:B9:31");
        }
        assert!(MacAddress::parse("00:0C:29").is_err());
        assert!(MacAddress::parse("00:0C:29:3B:B9:ZZ").is_err());
    }
}
