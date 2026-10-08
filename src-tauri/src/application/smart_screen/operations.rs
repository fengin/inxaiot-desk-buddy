use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::{
    model::ScreenAsset,
    operation::{READ_ACTIONS, WRITE_ACTIONS, ScreenOperationInput},
};
use std::collections::BTreeSet;
pub fn validate_input(input: &ScreenOperationInput) -> AppResult<()> {
    if input.target_ids.is_empty()
        || input.target_ids.len() > 1000
        || input.target_ids.iter().collect::<BTreeSet<_>>().len() != input.target_ids.len()
    {
        return Err(AppError::InvalidConfig(
            "请选择 1 至 1000 台不同的屏".into(),
        ));
    }
    if !(1..=3).contains(&input.concurrency) {
        return Err(AppError::InvalidConfig("当前最多同时执行 3 台设备".into()));
    }
    if !READ_ACTIONS.contains(&input.action.as_str()) && !WRITE_ACTIONS.contains(&input.action.as_str()) {
        return Err(AppError::InvalidConfig("该设备操作尚未接入正式执行".into()));
    }
    Ok(())
}
pub fn same_target(before: &ScreenAsset, after: &ScreenAsset) -> bool {
    before.id == after.id
        && before.source == after.source
        && before.fields.ip == after.fields.ip
        && crate::domain::smart_screen::rules::normalize_mac(&before.fields.mac)
            == crate::domain::smart_screen::rules::normalize_mac(&after.fields.mac)
        && before.fields.size == after.fields.size
}
