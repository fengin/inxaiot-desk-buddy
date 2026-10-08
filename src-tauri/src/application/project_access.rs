use crate::application::ports::project_access::ProjectAccessPort;
use crate::core::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectAccessRequirement {
    Configured,
    ActiveSession,
    /// 平台只读操作，不要求共享库可用。
    PlatformRead,
    /// 平台会话和共享数据库可用；业务表由各业务检查。
    Ready,
}

pub async fn require_project_access<P: ProjectAccessPort>(
    port: &P,
    project_id: &str,
    requirement: ProjectAccessRequirement,
) -> AppResult<()> {
    if project_id.trim().is_empty() {
        return Err(AppError::InvalidConfig("本地项目 ID 不能为空".into()));
    }
    port.require_project_access(project_id, requirement).await
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::{ProjectAccessRequirement, require_project_access};
    use crate::application::ports::project_access::ProjectAccessPort;
    use crate::core::error::{AppError, AppResult};

    struct FakePort {
        requests: Mutex<Vec<(String, ProjectAccessRequirement)>>,
        allow: bool,
    }

    impl ProjectAccessPort for FakePort {
        async fn require_project_access(
            &self,
            project_id: &str,
            requirement: ProjectAccessRequirement,
        ) -> AppResult<()> {
            self.requests
                .lock()
                .expect("requests")
                .push((project_id.into(), requirement));
            self.allow
                .then_some(())
                .ok_or_else(|| AppError::Authentication("会话已过期".into()))
        }
    }

    #[tokio::test]
    async fn guard_rejects_empty_project_and_propagates_requirement() {
        let port = FakePort {
            requests: Mutex::new(Vec::new()),
            allow: true,
        };
        assert!(
            require_project_access(&port, "", ProjectAccessRequirement::Ready)
                .await
                .is_err()
        );
        require_project_access(&port, "project", ProjectAccessRequirement::ActiveSession)
            .await
            .expect("allowed");
        assert_eq!(
            port.requests.lock().expect("requests").as_slice(),
            &[("project".into(), ProjectAccessRequirement::ActiveSession)]
        );
    }
}
