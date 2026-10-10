use crate::core::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NtpPatch {
    pub server: String,
}
impl NtpPatch {
    pub fn server(&self) -> &str {
        self.server.trim()
    }
    pub fn validate(&self) -> AppResult<()> {
        let server = self.server();
        if server.is_empty() || server.parse::<std::net::IpAddr>().is_ok() {
            return Ok(());
        }
        let host = server.strip_suffix('.').unwrap_or(server);
        if server.len() <= 253
            && !host.is_empty()
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && !label.starts_with('-')
                    && !label.ends_with('-')
            })
            && !host.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        {
            return Ok(());
        }
        Err(AppError::InvalidConfig(
            "NTP 服务器请填写有效 IP 或域名，不包含协议、端口或路径；留空恢复固件默认".into(),
        ))
    }
    pub fn matches(&self, config: &NtpConfig) -> bool {
        self.server() == config.server && config.auto_time
    }
    pub fn activation_required(&self, config: &NtpConfig, same_boot_verified: bool) -> bool {
        !self.matches(config) || !same_boot_verified
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NtpConfig {
    pub server: String,
    pub auto_time: bool,
    pub auto_time_zone: bool,
    pub time_zone: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NtpRead {
    pub screen_id: String,
    pub read_at: String,
    pub config: Option<NtpConfig>,
    pub capabilities: Option<serde_json::Value>,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_hosts_and_explicit_default_are_allowed_but_command_text_is_rejected() {
        for server in [
            "",
            "  ",
            "192.168.3.142",
            "time.internal",
            "ntp",
            "pool.ntp.org.",
            "2001:db8::1",
        ] {
            assert!(
                NtpPatch {
                    server: server.into()
                }
                .validate()
                .is_ok(),
                "{server}"
            );
        }
        for server in [
            "http://ntp",
            "192.168.3.142:123",
            "ntp/path",
            "ntp;reboot",
            "$(id)",
            "ntp\nserver",
            "-ntp",
            "ntp-",
            "a..b",
            "999.9.9.9",
            "[2001:db8::1]",
        ] {
            assert!(
                NtpPatch {
                    server: server.into()
                }
                .validate()
                .is_err(),
                "{server}"
            );
        }
    }
    #[test]
    fn same_address_with_disabled_automatic_time_still_requires_change() {
        let patch = NtpPatch {
            server: "192.168.3.142".into(),
        };
        let mut config = NtpConfig {
            server: patch.server.clone(),
            auto_time: false,
            auto_time_zone: true,
            time_zone: "Asia/Shanghai".into(),
        };
        assert!(!patch.matches(&config));
        config.auto_time = true;
        assert!(patch.matches(&config));
    }
    #[test]
    fn equal_persisted_settings_without_same_boot_evidence_still_require_activation() {
        let patch = NtpPatch {
            server: "192.168.3.142".into(),
        };
        let config = NtpConfig {
            server: patch.server.clone(),
            auto_time: true,
            auto_time_zone: true,
            time_zone: "Asia/Shanghai".into(),
        };
        assert!(patch.activation_required(&config, false));
        assert!(!patch.activation_required(&config, true));
        assert!(
            NtpPatch {
                server: "ntp.internal".into()
            }
            .activation_required(&config, true)
        );
    }
}
