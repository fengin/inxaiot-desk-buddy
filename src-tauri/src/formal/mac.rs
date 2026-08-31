use super::error::{FormalError, FormalResult};
use crate::domain::aio::mac::MacAddress;

pub fn normalize_mac(value: &str) -> FormalResult<String> {
    MacAddress::parse(value)
        .map(|mac| mac.normalized().to_string())
        .map_err(|_| FormalError::InvalidConfig("MAC地址格式无效".into()))
}

pub fn display_mac(normalized: &str) -> FormalResult<String> {
    MacAddress::parse(normalized)
        .map(|mac| mac.display())
        .map_err(|_| FormalError::InvalidConfig("MAC地址格式无效".into()))
}

#[cfg(test)]
mod tests {
    use super::{display_mac, normalize_mac};

    #[test]
    fn mac_identity_ignores_separators_and_case() {
        for input in [
            "00:0c:29:3b:b9:31",
            "00-0C-29-3B-B9-31",
            "000c.293b.b931",
            "00 0c 29 3b b9 31",
        ] {
            assert_eq!(normalize_mac(input).expect("normalize"), "000C293BB931");
        }
        assert_eq!(
            display_mac("000C293BB931").expect("display"),
            "00:0C:29:3B:B9:31"
        );
        assert!(normalize_mac("00:0C:29").is_err());
        assert!(normalize_mac("00:0C:29:3B:B9:ZZ").is_err());
    }
}
