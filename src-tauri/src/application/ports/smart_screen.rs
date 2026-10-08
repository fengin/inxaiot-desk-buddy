use crate::core::error::AppResult;
use crate::domain::smart_screen::model::{ScreenFields, ScreenSnapshot};

#[allow(async_fn_in_trait)]
pub trait ScreenAssetsPort: Send + Sync {
    async fn snapshot(&self, project: &str, refresh: bool) -> AppResult<ScreenSnapshot>;
    async fn select_business_project(&self, project: &str, business: &str) -> AppResult<()>;
    async fn save_local(
        &self,
        project: &str,
        fields: &ScreenFields,
        id: Option<&str>,
        revision: Option<u64>,
    ) -> AppResult<String>;
    async fn import_local(&self, project: &str, fields: &[ScreenFields]) -> AppResult<Vec<String>>;
    async fn remove_local(&self, project: &str, id: &str) -> AppResult<()>;
    async fn save_draft(
        &self,
        project: &str,
        id: &str,
        fields: &ScreenFields,
        revision: u64,
        asset_revision: u64,
    ) -> AppResult<()>;
    async fn discard_draft(&self, project: &str, id: &str, revision: u64) -> AppResult<()>;
}
