use crate::application::ports::smart_screen::ScreenAssetsPort;
use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::{
    model::{ScreenFields, ScreenSnapshot},
    rules::validate_fields,
};

pub async fn load<P: ScreenAssetsPort>(
    port: &P,
    project: &str,
    refresh: bool,
) -> AppResult<ScreenSnapshot> {
    if project.is_empty() {
        return Err(AppError::InvalidConfig("请选择项目".into()));
    }
    port.snapshot(project, refresh).await
}
pub async fn save<P: ScreenAssetsPort>(
    port: &P,
    project: &str,
    fields: &ScreenFields,
    id: Option<&str>,
    revision: Option<u64>,
) -> AppResult<String> {
    validate_fields(fields, false)?;
    port.save_local(project, fields, id, revision).await
}
pub async fn import<P: ScreenAssetsPort>(
    port: &P,
    project: &str,
    fields: &[ScreenFields],
) -> AppResult<Vec<String>> {
    for item in fields {
        validate_fields(item, false)?;
    }
    port.import_local(project, fields).await
}
