use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use time::OffsetDateTime;

use crate::application::ports::project_access::ProjectAccessPort;
use crate::application::ports::project_management::{
    HostKeyManagementPort, ProjectManagementPort, ReleaseProfileManagementPort,
};
use crate::application::ports::remote_session::{
    HostKeyIdentity, HostKeyPolicy, RemoteAuth, RemoteConnection, RemoteConnector, RemoteTarget,
};
use crate::application::project_access::ProjectAccessRequirement;
use crate::core::error::{AppError, AppResult};
use crate::core::secret::SecretValue;
use crate::domain::aio::release_profile::{
    ReleaseProfileCredentials, ReleaseProfileDraft, ReleaseProfileValues, ReleaseProfileView,
};
use crate::domain::common::project::{
    ConfirmHostKeyRequest, DatabaseConnectionState, HostKeyCaptureRequest, HostKeyObservation,
    HostKeyState, PlatformLoginChallenge, PlatformLoginRequest, ProjectConnectionState,
    ProjectConnectionTestRequest, ProjectConnectionTestResult, ProjectInput, ProjectOverview,
    ProjectRecord, ProjectSessionState, ProjectSessionView, is_private_network_host,
};
use crate::formal::app_state::FormalAppState;
use crate::formal::credential_crypto::ReleaseCredentials;
use crate::formal::error::FormalError;
use crate::formal::platform_auth::{PlatformAuthAdapter, PlatformLoginSpec};
use crate::formal::project_repository::{
    CreateLocalProject, LocalProjectRecord, LocalProjectRepository, UpdateLocalProject,
};
use crate::formal::release_profile_repository::{
    ReleaseProfileRecord, ReleaseProfileRepository,
    ReleaseProfileValues as StoredReleaseProfileValues, ReleaseProfileWrite,
};
use crate::formal::runtime_registry::ConnectionHealth;
use crate::formal::workbench_store::{WorkbenchSchemaStatus, WorkbenchStore};
use crate::infrastructure::database::{DatabaseTlsMode, DualMySqlPools, MySqlProjectConfig};
use crate::infrastructure::local_sqlite::host_key_repository::{HostKeyRecord, HostKeyRepository};
use crate::infrastructure::remote::{RusshConnector, validate_private_key_algorithm};

pub struct Stage75Adapter<'a> {
    state: &'a FormalAppState,
}

impl<'a> Stage75Adapter<'a> {
    pub fn new(state: &'a FormalAppState) -> Self {
        Self { state }
    }

    fn register_secrets(&self, values: impl IntoIterator<Item = String>) {
        if self
            .state
            .task_event_pipeline
            .register_secrets(values)
            .is_err()
        {
            tracing::error!("register project sensitive values failed");
        }
    }

    fn projects(&self) -> LocalProjectRepository {
        LocalProjectRepository::new(
            self.state.local_store.pool().clone(),
            self.state.secret_store.clone(),
        )
    }

    async fn session_view(&self, project_id: &str) -> AppResult<ProjectSessionView> {
        match self.projects().load_session(project_id).await {
            Ok(session) => {
                let expired = session
                    .session
                    .expires_at
                    .as_deref()
                    .and_then(|value| value.parse::<i64>().ok())
                    .is_some_and(|expires| expires <= OffsetDateTime::now_utc().unix_timestamp());
                Ok(ProjectSessionView {
                    local_project_id: project_id.into(),
                    username: Some(session.session.username),
                    state: if expired {
                        ProjectSessionState::Expired
                    } else {
                        ProjectSessionState::Active
                    },
                    expires_at: session.session.expires_at,
                    updated_at: Some(session.session.updated_at),
                })
            }
            Err(FormalError::NotFound(_)) => Ok(missing_session(project_id)),
            Err(error) => Err(map_formal_error(error)),
        }
    }

    async fn overview(&self, record: LocalProjectRecord) -> AppResult<ProjectOverview> {
        let runtime = self.state.runtime_registry.get(&record.id).await;
        let health = match &runtime {
            Some(runtime) => runtime.health().await,
            None => ConnectionHealth::Closed,
        };
        let session = self.session_view(&record.id).await?;
        let (database_state, connection_state, status_message) = match health {
            ConnectionHealth::Connecting => (
                DatabaseConnectionState::Disconnected,
                ProjectConnectionState::Connecting,
                "正在连接项目数据库".into(),
            ),
            ConnectionHealth::Ready => match session.state {
                ProjectSessionState::Active => (
                    DatabaseConnectionState::Connected,
                    ProjectConnectionState::Ready,
                    "数据库与平台会话已就绪".into(),
                ),
                ProjectSessionState::Expired => (
                    DatabaseConnectionState::Connected,
                    ProjectConnectionState::SessionExpired,
                    "平台会话已过期".into(),
                ),
                ProjectSessionState::Missing => (
                    DatabaseConnectionState::Connected,
                    ProjectConnectionState::LoginRequired,
                    "数据库已连接，需要登录平台".into(),
                ),
            },
            ConnectionHealth::Degraded => (
                DatabaseConnectionState::Failed,
                ProjectConnectionState::ConnectionFailed,
                "项目数据库连接或结构检查失败".into(),
            ),
            ConnectionHealth::Closed => (
                DatabaseConnectionState::Disconnected,
                ProjectConnectionState::Disconnected,
                "尚未连接当前项目".into(),
            ),
        };
        Ok(ProjectOverview {
            project: map_project(record),
            connection_state,
            database_state,
            schema_state: None,
            session: (session.state != ProjectSessionState::Missing).then_some(session),
            connection_encrypted: false,
            status_message,
        })
    }

    async fn close_runtime_if_open(&self, project_id: &str) {
        if self.state.runtime_registry.get(project_id).await.is_some() {
            let _ = self.state.runtime_registry.close(project_id).await;
        }
    }

    async fn ensure_project_mutable(&self, project_id: &str) -> AppResult<()> {
        if self
            .state
            .task_repository
            .has_active_for_project(project_id)
            .await?
        {
            return Err(AppError::Conflict(
                "项目存在活动任务，任务进入终态前不能编辑或删除".into(),
            ));
        }
        Ok(())
    }

    async fn pools_and_schema(
        &self,
        project_id: &str,
    ) -> AppResult<(Arc<DualMySqlPools>, WorkbenchSchemaStatus)> {
        let runtime = self
            .state
            .runtime_registry
            .open(project_id)
            .await
            .map_err(map_formal_error)?;
        runtime.set_health(ConnectionHealth::Connecting).await;
        let result = runtime
            .database_or_try_init(|| async {
                let connection = self
                    .projects()
                    .connection_secrets(project_id)
                    .await
                    .map_err(map_formal_error)?;
                let config = mysql_config(
                    &ProjectInput {
                        name: connection.project.name,
                        platform_url: connection.project.platform_url,
                        db_host: connection.project.db_host,
                        db_port: connection.project.db_port,
                        db_user: connection.project.db_user,
                        db_password: Some(connection.db_password),
                        business_db: connection.project.business_db,
                        workbench_db: connection.project.workbench_db,
                    },
                    None,
                )?;
                let pools = DualMySqlPools::connect(&config).await?;
                let report = pools.probe(&config).await?;
                if !report.missing_required_columns.is_empty() {
                    pools.close().await;
                    return Err(AppError::InvalidConfig(format!(
                        "平台一体机表缺少字段：{}",
                        report.missing_required_columns.join("、")
                    )));
                }
                self.projects()
                    .touch_opened(project_id)
                    .await
                    .map_err(map_formal_error)?;
                Ok(pools)
            })
            .await;
        let pools = match result {
            Ok(pools) => pools,
            Err(error) => {
                runtime.set_health(ConnectionHealth::Degraded).await;
                return Err(error);
            }
        };
        let project = self
            .projects()
            .get(project_id)
            .await
            .map_err(map_formal_error)?;
        let schema = WorkbenchStore::new(pools.workbench.clone())
            .schema_status(&project.workbench_db)
            .await
            .map_err(map_formal_error)?;
        runtime
            .set_health(if schema.is_ready() {
                ConnectionHealth::Ready
            } else {
                ConnectionHealth::Degraded
            })
            .await;
        Ok((pools, schema))
    }

    async fn ready_pools(&self, project_id: &str) -> AppResult<Arc<DualMySqlPools>> {
        let (pools, schema) = self.pools_and_schema(project_id).await?;
        if !schema.is_ready() {
            return Err(AppError::Conflict(format!(
                "{}；请先显式初始化或升级工作台数据库",
                schema.message
            )));
        }
        Ok(pools)
    }

    async fn require_active_session(&self, project_id: &str) -> AppResult<ProjectSessionView> {
        let session = self.session_view(project_id).await?;
        if session.state != ProjectSessionState::Active {
            return Err(AppError::Authentication(
                "平台会话缺失或已过期，当前项目业务操作已阻止".into(),
            ));
        }
        Ok(session)
    }
}

impl ProjectManagementPort for Stage75Adapter<'_> {
    async fn list_projects(&self) -> AppResult<Vec<ProjectOverview>> {
        let mut overviews = Vec::new();
        for project in self.projects().list().await.map_err(map_formal_error)? {
            overviews.push(self.overview(project).await?);
        }
        Ok(overviews)
    }

    async fn create_project(&self, input: ProjectInput) -> AppResult<ProjectOverview> {
        self.register_secrets(input.db_password.clone());
        let record = self
            .projects()
            .create(CreateLocalProject {
                name: input.name,
                platform_url: input.platform_url,
                db_host: input.db_host,
                db_port: input.db_port,
                db_user: input.db_user,
                db_password: input.db_password.unwrap_or_default(),
                business_db: input.business_db,
                workbench_db: input.workbench_db,
            })
            .await
            .map_err(map_formal_error)?;
        self.overview(record).await
    }

    async fn update_project(
        &self,
        project_id: &str,
        input: ProjectInput,
    ) -> AppResult<ProjectOverview> {
        self.ensure_project_mutable(project_id).await?;
        if let Some(new_password) = input
            .db_password
            .as_deref()
            .filter(|password| !password.is_empty())
        {
            let existing = self
                .projects()
                .connection_secrets(project_id)
                .await
                .map_err(map_formal_error)?;
            if new_password != existing.db_password {
                let pools = self.ready_pools(project_id).await?;
                match ReleaseProfileRepository::new(pools.workbench.clone())
                    .get(&existing.db_password, "default")
                    .await
                {
                    Ok(_) => {
                        return Err(AppError::Conflict(
                            "当前发布凭据仍由原数据库密码加密；稳定项目主密钥迁移完成前禁止修改数据库密码"
                                .into(),
                        ));
                    }
                    Err(FormalError::NotFound(_)) => {}
                    Err(error) => return Err(map_formal_error(error)),
                }
            }
        }
        self.register_secrets(input.db_password.clone());
        self.close_runtime_if_open(project_id).await;
        let record = self
            .projects()
            .update(
                project_id,
                UpdateLocalProject {
                    name: input.name,
                    platform_url: input.platform_url,
                    db_host: input.db_host,
                    db_port: input.db_port,
                    db_user: input.db_user,
                    db_password: input.db_password,
                    business_db: input.business_db,
                    workbench_db: input.workbench_db,
                },
            )
            .await
            .map_err(map_formal_error)?;
        self.overview(record).await
    }

    async fn delete_project(&self, project_id: &str) -> AppResult<()> {
        self.ensure_project_mutable(project_id).await?;
        self.close_runtime_if_open(project_id).await;
        self.projects()
            .delete(project_id)
            .await
            .map_err(map_formal_error)
    }

    async fn test_project_connection(
        &self,
        request: ProjectConnectionTestRequest,
    ) -> AppResult<ProjectConnectionTestResult> {
        let saved_password = if request
            .project
            .db_password
            .as_deref()
            .is_none_or(str::is_empty)
        {
            let project_id = request
                .existing_project_id
                .as_deref()
                .ok_or_else(|| AppError::InvalidConfig("测试连接缺少数据库密码".into()))?;
            Some(
                self.projects()
                    .connection_secrets(project_id)
                    .await
                    .map_err(map_formal_error)?
                    .db_password,
            )
        } else {
            None
        };
        self.register_secrets(
            request
                .project
                .db_password
                .clone()
                .into_iter()
                .chain(saved_password.clone()),
        );
        let config = mysql_config(&request.project, saved_password)?;
        let pools = DualMySqlPools::connect(&config).await?;
        let report = match pools.probe(&config).await {
            Ok(report) => report,
            Err(error) => {
                pools.close().await;
                return Err(error);
            }
        };
        let schema = WorkbenchStore::new(pools.workbench.clone())
            .schema_status(&config.workbench_schema)
            .await
            .map_err(map_formal_error)?;
        pools.close().await;
        let platform_schema_compatible = report.missing_required_columns.is_empty();
        Ok(ProjectConnectionTestResult {
            successful: platform_schema_compatible,
            platform_database_connected: true,
            workbench_database_connected: true,
            platform_schema_compatible,
            workbench_schema_state: schema.state.as_str().into(),
            workbench_schema_message: schema.message,
            mysql_version: report.server_version,
            connection_encrypted: report.tls_cipher.is_some(),
            message: if platform_schema_compatible {
                "双数据库连接和平台 Schema 探测通过".into()
            } else {
                format!(
                    "平台一体机表缺少字段：{}",
                    report.missing_required_columns.join("、")
                )
            },
        })
    }

    async fn switch_project(&self, project_id: &str) -> AppResult<ProjectOverview> {
        let (_, schema) = self.pools_and_schema(project_id).await?;
        let record = self
            .projects()
            .get(project_id)
            .await
            .map_err(map_formal_error)?;
        let session = self.session_view(project_id).await?;
        let (connection_state, message) = if !schema.is_ready() {
            (
                ProjectConnectionState::SchemaRequired,
                schema.message.clone(),
            )
        } else {
            match session.state {
                ProjectSessionState::Active => (
                    ProjectConnectionState::Ready,
                    "数据库与平台会话已就绪".into(),
                ),
                ProjectSessionState::Expired => (
                    ProjectConnectionState::SessionExpired,
                    "平台会话已过期，请重新登录".into(),
                ),
                ProjectSessionState::Missing => (
                    ProjectConnectionState::LoginRequired,
                    "数据库已连接，请登录项目平台".into(),
                ),
            }
        };
        Ok(ProjectOverview {
            project: map_project(record),
            connection_state,
            database_state: DatabaseConnectionState::Connected,
            schema_state: Some(schema.state.as_str().into()),
            session: (session.state != ProjectSessionState::Missing).then_some(session),
            connection_encrypted: false,
            status_message: message,
        })
    }

    async fn create_login_challenge(&self, project_id: &str) -> AppResult<PlatformLoginChallenge> {
        let project = self
            .projects()
            .get(project_id)
            .await
            .map_err(map_formal_error)?;
        let session_uuid = uuid::Uuid::now_v7().to_string();
        let image = fetch_captcha(&project.platform_url, &session_uuid).await;
        Ok(PlatformLoginChallenge {
            session_uuid,
            captcha_image_data_url: image,
            requires_captcha: true,
            expires_at_epoch_seconds: OffsetDateTime::now_utc().unix_timestamp() + 300,
        })
    }

    async fn login_project(
        &self,
        project_id: &str,
        request: PlatformLoginRequest,
    ) -> AppResult<ProjectSessionView> {
        self.register_secrets([request.password.clone()]);
        let project = self
            .projects()
            .get(project_id)
            .await
            .map_err(map_formal_error)?;
        let auth = PlatformAuthAdapter::new(Duration::from_secs(15)).map_err(map_formal_error)?;
        let session = auth
            .login(&PlatformLoginSpec {
                base_url: project.platform_url,
                principal: request.username,
                password: request.password,
                session_uuid: request.session_uuid,
                image_code: request.image_code,
                timeout: Duration::from_secs(15),
            })
            .await
            .map_err(map_formal_error)?;
        let expires_at = session
            .expires_at
            .map(|expires| expires.unix_timestamp().to_string());
        self.projects()
            .save_session(
                project_id,
                &session.principal,
                &session.access_token,
                expires_at,
            )
            .await
            .map_err(map_formal_error)?;
        self.session_view(project_id).await
    }

    async fn get_project_session(&self, project_id: &str) -> AppResult<ProjectSessionView> {
        self.projects()
            .get(project_id)
            .await
            .map_err(map_formal_error)?;
        self.session_view(project_id).await
    }

    async fn check_project_session(&self, project_id: &str) -> AppResult<ProjectSessionView> {
        let stored = match self.projects().load_session(project_id).await {
            Ok(stored) => stored,
            Err(FormalError::NotFound(_)) => return Ok(missing_session(project_id)),
            Err(error) => return Err(map_formal_error(error)),
        };
        self.register_secrets([stored.access_token.clone()]);
        let session = self.session_view(project_id).await?;
        if session.state == ProjectSessionState::Expired {
            self.projects()
                .clear_session(project_id)
                .await
                .map_err(map_formal_error)?;
            return Ok(ProjectSessionView {
                state: ProjectSessionState::Expired,
                ..session
            });
        }
        let project = self
            .projects()
            .get(project_id)
            .await
            .map_err(map_formal_error)?;
        let auth = PlatformAuthAdapter::new(Duration::from_secs(15)).map_err(map_formal_error)?;
        if !auth
            .validate_access_token(&project.platform_url, &stored.access_token)
            .await
            .map_err(map_formal_error)?
        {
            self.projects()
                .clear_session(project_id)
                .await
                .map_err(map_formal_error)?;
            return Ok(ProjectSessionView {
                local_project_id: project_id.into(),
                username: Some(stored.session.username),
                state: ProjectSessionState::Expired,
                expires_at: stored.session.expires_at,
                updated_at: Some(stored.session.updated_at),
            });
        }
        Ok(session)
    }

    async fn logout_project(&self, project_id: &str) -> AppResult<ProjectSessionView> {
        self.projects()
            .clear_session(project_id)
            .await
            .map_err(map_formal_error)?;
        Ok(missing_session(project_id))
    }
}

impl ProjectAccessPort for Stage75Adapter<'_> {
    async fn require_project_access(
        &self,
        project_id: &str,
        requirement: ProjectAccessRequirement,
    ) -> AppResult<()> {
        self.projects()
            .get(project_id)
            .await
            .map_err(map_formal_error)?;
        match requirement {
            ProjectAccessRequirement::Configured => {}
            ProjectAccessRequirement::ActiveSession => {
                self.require_active_session(project_id).await?;
            }
            ProjectAccessRequirement::Ready => {
                self.require_active_session(project_id).await?;
                self.ready_pools(project_id).await?;
            }
        }
        Ok(())
    }
}

impl ReleaseProfileManagementPort for Stage75Adapter<'_> {
    async fn get_release_profile(&self, project_id: &str) -> AppResult<Option<ReleaseProfileView>> {
        self.require_active_session(project_id).await?;
        let pools = self.ready_pools(project_id).await?;
        let connection = self
            .projects()
            .connection_secrets(project_id)
            .await
            .map_err(map_formal_error)?;
        match ReleaseProfileRepository::new(pools.workbench.clone())
            .get(&connection.db_password, "default")
            .await
        {
            Ok(record) => {
                self.register_secrets(
                    stored_release_secret_values(&record.credentials)
                        .chain([connection.db_password]),
                );
                Ok(Some(map_release_profile(record)))
            }
            Err(FormalError::NotFound(_)) => Ok(None),
            Err(error) => Err(map_formal_error(error)),
        }
    }

    async fn save_release_profile(
        &self,
        project_id: &str,
        draft: ReleaseProfileDraft,
    ) -> AppResult<ReleaseProfileView> {
        if let Some(private_key) = draft
            .credentials
            .ssh_private_key
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            validate_private_key_algorithm(private_key)?;
        }
        self.register_secrets(profile_draft_secret_values(&draft.credentials));
        let session = self.require_active_session(project_id).await?;
        let operator_name = session
            .username
            .ok_or_else(|| AppError::Authentication("平台会话缺少用户名".into()))?;
        let pools = self.ready_pools(project_id).await?;
        let connection = self
            .projects()
            .connection_secrets(project_id)
            .await
            .map_err(map_formal_error)?;
        let record = ReleaseProfileRepository::new(pools.workbench.clone())
            .save(
                &connection.db_password,
                ReleaseProfileWrite {
                    profile_key: "default".into(),
                    values: StoredReleaseProfileValues {
                        env_template: draft.values.env_template,
                        compose_template: draft.values.compose_template,
                        platform_host: draft.values.platform_host,
                        platform_api_port: draft.values.platform_api_port,
                        platform_mqtt_host: draft.values.platform_mqtt_host,
                        platform_mqtt_port: draft.values.platform_mqtt_port,
                        ssh_port: draft.values.ssh_port,
                        ssh_timeout_seconds: draft.values.ssh_timeout_seconds,
                        aio_data_root: draft.values.aio_data_root,
                        aio_deploy_root: draft.values.aio_deploy_root,
                    },
                    credentials: ReleaseCredentials {
                        platform_auth_key: draft.credentials.platform_auth_key,
                        platform_mqtt_user: draft.credentials.platform_mqtt_user,
                        platform_mqtt_password: draft.credentials.platform_mqtt_password,
                        aio_mqtt_user: draft.credentials.aio_mqtt_user,
                        aio_mqtt_password: draft.credentials.aio_mqtt_password,
                        ssh_user: draft.credentials.ssh_user,
                        ssh_password: draft.credentials.ssh_password,
                        ssh_private_key: draft.credentials.ssh_private_key,
                    },
                    expected_version: draft.expected_version,
                    operator_name,
                    instance_id: format!("desk-{}", std::process::id()),
                },
            )
            .await
            .map_err(map_formal_error)?;
        Ok(map_release_profile(record))
    }
}

impl HostKeyManagementPort for Stage75Adapter<'_> {
    async fn list_host_keys(&self, project_id: &str) -> AppResult<Vec<HostKeyObservation>> {
        self.require_active_session(project_id).await?;
        HostKeyRepository::new(self.state.local_store.pool().clone())
            .list(project_id)
            .await?
            .into_iter()
            .map(|record| Ok(map_host_key(record)))
            .collect()
    }

    async fn capture_host_key(
        &self,
        project_id: &str,
        request: HostKeyCaptureRequest,
    ) -> AppResult<HostKeyObservation> {
        self.require_active_session(project_id).await?;
        let (target, identity) = self.capture_identity(project_id, &request).await?;
        let known = HostKeyRepository::new(self.state.local_store.pool().clone())
            .get(project_id, &target)
            .await?;
        let (state, expected_fingerprint, accepted_at) = match known {
            Some(record) if record.identity == identity => (
                HostKeyState::Confirmed,
                Some(record.identity.fingerprint),
                Some(record.accepted_at),
            ),
            Some(record) => (
                HostKeyState::Changed,
                Some(record.identity.fingerprint),
                Some(record.accepted_at),
            ),
            None => (HostKeyState::Unconfirmed, None, None),
        };
        Ok(HostKeyObservation {
            host: target.host,
            port: target.port,
            algorithm: identity.algorithm,
            fingerprint: identity.fingerprint,
            state,
            expected_fingerprint,
            accepted_at,
        })
    }

    async fn confirm_host_key(
        &self,
        project_id: &str,
        request: ConfirmHostKeyRequest,
    ) -> AppResult<HostKeyObservation> {
        self.require_active_session(project_id).await?;
        let capture = HostKeyCaptureRequest {
            host: request.host.clone(),
            port: Some(request.port),
        };
        let (target, actual) = self.capture_identity(project_id, &capture).await?;
        if actual.algorithm != request.algorithm || actual.fingerprint != request.fingerprint {
            return Err(AppError::Conflict(
                "主机密钥在捕获与确认之间发生变化，请重新捕获".into(),
            ));
        }
        let record = HostKeyRepository::new(self.state.local_store.pool().clone())
            .confirm(project_id, &target, &actual, request.replace_changed)
            .await?;
        Ok(map_host_key(record))
    }
}

impl Stage75Adapter<'_> {
    async fn capture_identity(
        &self,
        project_id: &str,
        request: &HostKeyCaptureRequest,
    ) -> AppResult<(RemoteTarget, HostKeyIdentity)> {
        let pools = self.ready_pools(project_id).await?;
        let connection = self
            .projects()
            .connection_secrets(project_id)
            .await
            .map_err(map_formal_error)?;
        let profile = ReleaseProfileRepository::new(pools.workbench.clone())
            .get(&connection.db_password, "default")
            .await
            .map_err(map_formal_error)?;
        self.register_secrets(
            stored_release_secret_values(&profile.credentials)
                .chain([connection.db_password.clone()]),
        );
        let target = RemoteTarget {
            host: request.host.trim().into(),
            port: request.port.unwrap_or(profile.values.ssh_port),
            connect_timeout: Duration::from_secs(u64::from(profile.values.ssh_timeout_seconds)),
        };
        let auth = if let Some(private_key) = profile
            .credentials
            .ssh_private_key
            .filter(|value| !value.is_empty())
        {
            RemoteAuth::PrivateKey {
                username: profile.credentials.ssh_user,
                private_key: SecretValue::new(private_key),
                passphrase: None,
            }
        } else {
            RemoteAuth::Password {
                username: profile.credentials.ssh_user,
                password: SecretValue::new(
                    profile
                        .credentials
                        .ssh_password
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| AppError::InvalidConfig("SSH凭据为空".into()))?,
                ),
            }
        };
        let connection = RusshConnector::default()
            .connect(&target, &auth, HostKeyPolicy::Capture)
            .await?;
        let identity = connection.host_key().clone();
        connection.disconnect().await?;
        Ok((target, identity))
    }
}

fn profile_draft_secret_values(
    credentials: &ReleaseProfileCredentials,
) -> impl Iterator<Item = String> {
    let mut values = vec![
        credentials.platform_auth_key.clone(),
        credentials.platform_mqtt_user.clone(),
        credentials.platform_mqtt_password.clone(),
        credentials.aio_mqtt_user.clone(),
        credentials.aio_mqtt_password.clone(),
        credentials.ssh_user.clone(),
    ];
    values.extend(credentials.ssh_password.clone());
    values.extend(credentials.ssh_private_key.clone());
    values.into_iter()
}

fn stored_release_secret_values(credentials: &ReleaseCredentials) -> impl Iterator<Item = String> {
    let mut values = vec![
        credentials.platform_auth_key.clone(),
        credentials.platform_mqtt_user.clone(),
        credentials.platform_mqtt_password.clone(),
        credentials.aio_mqtt_user.clone(),
        credentials.aio_mqtt_password.clone(),
        credentials.ssh_user.clone(),
    ];
    values.extend(credentials.ssh_password.clone());
    values.extend(credentials.ssh_private_key.clone());
    values.into_iter()
}

fn map_project(project: LocalProjectRecord) -> ProjectRecord {
    ProjectRecord {
        id: project.id,
        name: project.name,
        platform_url: project.platform_url,
        db_host: project.db_host,
        db_port: project.db_port,
        db_user: project.db_user,
        business_db: project.business_db,
        workbench_db: project.workbench_db,
        last_opened_at: project.last_opened_at,
    }
}

fn missing_session(project_id: &str) -> ProjectSessionView {
    ProjectSessionView {
        local_project_id: project_id.into(),
        username: None,
        state: ProjectSessionState::Missing,
        expires_at: None,
        updated_at: None,
    }
}

fn mysql_config(
    input: &ProjectInput,
    saved_password: Option<String>,
) -> AppResult<MySqlProjectConfig> {
    let password = input
        .db_password
        .clone()
        .filter(|value| !value.is_empty())
        .or(saved_password)
        .ok_or_else(|| AppError::InvalidConfig("数据库密码不能为空".into()))?;
    Ok(MySqlProjectConfig {
        host: input.db_host.trim().into(),
        port: input.db_port,
        username: input.db_user.trim().into(),
        password: SecretValue::new(password),
        platform_schema: input.business_db.trim().into(),
        workbench_schema: input.workbench_db.trim().into(),
        connect_timeout: Duration::from_secs(10),
        tls_mode: if is_private_network_host(&input.db_host) {
            DatabaseTlsMode::Preferred
        } else {
            DatabaseTlsMode::Required
        },
    })
}

fn map_release_profile(record: ReleaseProfileRecord) -> ReleaseProfileView {
    ReleaseProfileView {
        profile_key: record.profile_key,
        values: ReleaseProfileValues {
            env_template: record.values.env_template,
            compose_template: record.values.compose_template,
            platform_host: record.values.platform_host,
            platform_api_port: record.values.platform_api_port,
            platform_mqtt_host: record.values.platform_mqtt_host,
            platform_mqtt_port: record.values.platform_mqtt_port,
            ssh_port: record.values.ssh_port,
            ssh_timeout_seconds: record.values.ssh_timeout_seconds,
            aio_data_root: record.values.aio_data_root,
            aio_deploy_root: record.values.aio_deploy_root,
        },
        credentials: ReleaseProfileCredentials {
            platform_auth_key: record.credentials.platform_auth_key,
            platform_mqtt_user: record.credentials.platform_mqtt_user,
            platform_mqtt_password: record.credentials.platform_mqtt_password,
            aio_mqtt_user: record.credentials.aio_mqtt_user,
            aio_mqtt_password: record.credentials.aio_mqtt_password,
            ssh_user: record.credentials.ssh_user,
            ssh_password: record.credentials.ssh_password,
            ssh_private_key: record.credentials.ssh_private_key,
        },
        version: record.version,
        updated_by: record.updated_by,
        updated_at: record.updated_at.to_string(),
    }
}

fn map_host_key(record: HostKeyRecord) -> HostKeyObservation {
    HostKeyObservation {
        host: record.host,
        port: record.port,
        algorithm: record.identity.algorithm,
        fingerprint: record.identity.fingerprint.clone(),
        state: HostKeyState::Confirmed,
        expected_fingerprint: Some(record.identity.fingerprint),
        accepted_at: Some(record.accepted_at),
    }
}

async fn fetch_captcha(base_url: &str, session_uuid: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;
    let base_url = base_url.trim_end_matches('/');
    let urls = [
        format!("{base_url}/captcha.jpg?uuid={session_uuid}"),
        format!("{base_url}/edge/apis/captcha.jpg?uuid={session_uuid}"),
    ];
    for url in urls {
        let response = match client.get(url).send().await {
            Ok(response) if response.status().is_success() => response,
            _ => continue,
        };
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        let bytes = match response.bytes().await {
            Ok(bytes) if !bytes.is_empty() => bytes,
            _ => continue,
        };
        return Some(format!("data:{mime};base64,{}", STANDARD.encode(bytes)));
    }
    None
}

fn map_formal_error(error: FormalError) -> AppError {
    match error {
        FormalError::InvalidConfig(message) => AppError::InvalidConfig(message),
        FormalError::Conflict(message) => AppError::Conflict(message),
        FormalError::NotFound(message) => AppError::NotFound(message),
        FormalError::LocalDatabase(operation) => AppError::Database { operation },
        FormalError::SecretStore(operation) | FormalError::LocalIo(operation) => {
            AppError::Io { operation }
        }
    }
}
