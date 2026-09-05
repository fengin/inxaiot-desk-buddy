use serde::{Deserialize, Serialize};

use crate::core::error::AppResult;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInspectionSubmission {
    pub task_id: String,
}

#[allow(async_fn_in_trait)]
pub trait ServiceInspectionPort: Send + Sync {
    async fn submit(&self, project_id: &str, mac: &str) -> AppResult<ServiceInspectionSubmission>;
}
