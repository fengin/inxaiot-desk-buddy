use super::ntp::NtpPatch;
use crate::core::error::AppResult;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScreenSettingsPatch {
    pub adb: bool,
    pub time: bool,
    pub sleep_off: bool,
    pub ntp: Option<NtpPatch>,
}
impl Default for ScreenSettingsPatch {
    fn default() -> Self {
        Self {
            adb: true,
            time: true,
            sleep_off: true,
            ntp: None,
        }
    }
}
impl ScreenSettingsPatch {
    pub fn validate(&self) -> AppResult<()> {
        if let Some(ntp) = &self.ntp {
            ntp.validate()?;
        }
        Ok(())
    }
    pub fn selected(&self, code: &str) -> bool {
        match code {
            "adb" => self.adb,
            "time" => self.time,
            "sleepOff" => self.sleep_off,
            "ntp" => self.ntp.is_some(),
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenSettingItem {
    pub state: String,
    pub current: Value,
    pub target: Value,
    pub current_display: String,
    pub target_display: String,
    pub reason: String,
    pub reboot_required: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenSettingsRead {
    pub screen_id: String,
    pub read_at: String,
    pub items: BTreeMap<String, ScreenSettingItem>,
    pub pending_count: usize,
    pub blocked_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
