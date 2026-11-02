use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use inxaiot_desk_buddy_lib::application::ports::deployment_progress::{
    DeploymentProgressEvent, DeploymentProgressSink,
};
use inxaiot_desk_buddy_lib::application::ports::file_transfer::{
    DownloadRequest, FileTransferService, RemoteFileMetadata, TransferProgress,
    TransferProgressSink, UploadRequest,
};
use inxaiot_desk_buddy_lib::application::ports::remote_command::{
    ExecRequest, RemoteCommandExecutor, RemoteCommandResult, RemoteOutputSink,
};
use inxaiot_desk_buddy_lib::application::ports::remote_session::{
    HostKeyIdentity, RemoteConnection,
};
use inxaiot_desk_buddy_lib::core::error::{AppError, AppResult};
use inxaiot_desk_buddy_lib::domain::aio::deployment::{
    DeploymentImageInput, DeploymentMode, DeploymentPlan, DeploymentPlanInput,
};
use inxaiot_desk_buddy_lib::infrastructure::deployment_remote::{
    RemoteDeploymentConfig, RemoteDeploymentFiles,
    execute_connected_deployment_observed_with_checkpoint,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
enum ClockBehavior {
    Behind,
    Aligned,
    WriteRejected,
    ReadMalformed,
    VerificationStaysBehind,
}

#[derive(Clone, Debug)]
enum Recorded {
    Command { program: String, args: Vec<String> },
    Upload,
    Progress { stage: String, code: String },
}

impl Recorded {
    fn is_clock_read(&self) -> bool {
        matches!(self, Self::Command { program, args } if program == "date" && args == &["-u", "+%s%3N"])
    }

    fn is_clock_write(&self) -> bool {
        matches!(self, Self::Command { program, args } if program == "sh" && args.get(2).is_some_and(|value| value == "inx-clock-sync"))
    }

    fn agent_action(&self) -> Option<&str> {
        match self {
            Self::Command { program, args }
                if program == "sh"
                    && args
                        .first()
                        .is_some_and(|value| value.ends_with("/edge-node-agent.sh")) =>
            {
                args.get(1).map(String::as_str)
            }
            _ => None,
        }
    }
}

struct FakeRemote {
    behavior: ClockBehavior,
    synchronized: AtomicBool,
    reached_chmod: Arc<AtomicBool>,
    reached_clock_read: Arc<AtomicBool>,
    cancel_after_chmod: Option<CancellationToken>,
    history: Arc<Mutex<Vec<Recorded>>>,
    identity: HostKeyIdentity,
}

impl RemoteCommandExecutor for FakeRemote {
    async fn run(
        &self,
        request: &ExecRequest,
        cancellation: &CancellationToken,
        _: &dyn RemoteOutputSink,
    ) -> AppResult<RemoteCommandResult> {
        if cancellation.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let call = Recorded::Command {
            program: request.program.clone(),
            args: request.args.clone(),
        };
        self.history.lock().unwrap().push(call.clone());
        let mut stdout = String::new();
        let mut exit_status = 0;
        if call.is_clock_read() {
            self.reached_clock_read.store(true, Ordering::SeqCst);
            stdout = if matches!(self.behavior, ClockBehavior::ReadMalformed) {
                "不合法的远端时间\n".into()
            } else {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis();
                let aligned = matches!(self.behavior, ClockBehavior::Aligned)
                    || (self.synchronized.load(Ordering::SeqCst)
                        && !matches!(self.behavior, ClockBehavior::VerificationStaysBehind));
                format!("{}\n", if aligned { now } else { now - 120_000 })
            };
        } else if call.is_clock_write() {
            if matches!(self.behavior, ClockBehavior::WriteRejected) {
                exit_status = 1;
            } else {
                self.synchronized.store(true, Ordering::SeqCst);
            }
        } else if let Some(action) = call.agent_action() {
            stdout = format!(
                "{{\"step\":\"{action}\",\"status\":\"success\",\"message\":\"测试替身完成\"}}\n"
            );
        } else if request.program == "chmod" {
            self.reached_chmod.store(true, Ordering::SeqCst);
            if let Some(token) = &self.cancel_after_chmod {
                token.cancel();
            }
        }
        Ok(RemoteCommandResult {
            exit_status,
            stdout,
            stderr: if exit_status == 0 {
                String::new()
            } else {
                "校时权限不足".into()
            },
            duration_ms: 1,
        })
    }
}

impl FileTransferService for FakeRemote {
    async fn upload(
        &self,
        request: &UploadRequest,
        cancellation: &CancellationToken,
        progress: &dyn TransferProgressSink,
    ) -> AppResult<()> {
        if cancellation.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        self.history.lock().unwrap().push(Recorded::Upload);
        let total = std::fs::metadata(&request.local_path).unwrap().len();
        progress.emit(TransferProgress {
            transferred: total,
            total,
        })
    }

    async fn download(
        &self,
        _: &DownloadRequest,
        _: &CancellationToken,
        _: &dyn TransferProgressSink,
    ) -> AppResult<()> {
        panic!("部署校时流程不应下载文件")
    }

    async fn stat(&self, _: &str) -> AppResult<RemoteFileMetadata> {
        panic!("部署校时流程不应新增文件属性读取")
    }

    async fn rename(&self, _: &str, _: &str) -> AppResult<()> {
        panic!("部署校时流程不应新增远端文件移动")
    }

    async fn remove_file(&self, _: &str) -> AppResult<()> {
        panic!("部署校时流程不应新增单文件删除")
    }
}

impl RemoteConnection for FakeRemote {
    fn host_key(&self) -> &HostKeyIdentity {
        &self.identity
    }

    async fn disconnect(&self) -> AppResult<()> {
        Ok(())
    }
}

struct CaptureProgress(Arc<Mutex<Vec<Recorded>>>);

impl DeploymentProgressSink for CaptureProgress {
    fn emit(&self, event: DeploymentProgressEvent) -> AppResult<()> {
        self.0.lock().unwrap().push(Recorded::Progress {
            stage: event.stage,
            code: event.message_code,
        });
        Ok(())
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    plan: DeploymentPlan,
    files: RemoteDeploymentFiles,
    config: RemoteDeploymentConfig,
    remote: FakeRemote,
    progress: CaptureProgress,
}

impl Fixture {
    fn new(mode: DeploymentMode, behavior: ClockBehavior) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let write_file = |name: &str| {
            let path = directory.path().join(name);
            std::fs::write(&path, format!("隔离校时测试文件：{name}")).unwrap();
            path
        };
        let agent = write_file("edge-node-agent.sh");
        let artifact = write_file("release.tar");
        let env = write_file(".env");
        let host_info = write_file("host-info.json");
        let compose = write_file("docker-compose.yml");
        let plan = DeploymentPlan::build(DeploymentPlanInput {
            mode,
            target_macs: vec!["001122334455".into()],
            image_files: vec![DeploymentImageInput {
                service_name: "edge".into(),
                file_path: artifact.to_string_lossy().into_owned(),
                image_tag: "edge:clock-test".into(),
            }],
            artifact_path: artifact.to_string_lossy().into_owned(),
            artifact_name: "隔离校时测试".into(),
            artifact_version: "clock-test".into(),
            service_name: None,
            image_name: None,
            service_image_environment_variable: None,
            images: BTreeMap::new(),
            batch_size: 1,
            concurrency: 1,
        })
        .unwrap();
        let history = Arc::new(Mutex::new(Vec::new()));
        Self {
            _directory: directory,
            plan,
            files: RemoteDeploymentFiles {
                local_agent: agent,
                local_artifact: artifact,
                local_env: (mode != DeploymentMode::ServiceUpgrade).then_some(env),
                local_host_info: (mode != DeploymentMode::ServiceUpgrade).then_some(host_info),
                local_compose: (mode != DeploymentMode::ServiceUpgrade).then_some(compose),
            },
            config: RemoteDeploymentConfig {
                operation_id: "clock-workflow-test".into(),
                release_fingerprint: "clock-test".into(),
                mac_normalized: "001122334455".into(),
                data_root: "/opt/data".into(),
                deploy_root: "/opt/data/deploy".into(),
                platform_api_host: "platform.invalid".into(),
                platform_api_port: 8055,
                platform_mqtt_host: "platform.invalid".into(),
                platform_mqtt_port: 1883,
                allow_existing_ports: true,
                published_ports: vec![6002],
            },
            remote: FakeRemote {
                behavior,
                synchronized: AtomicBool::new(false),
                reached_chmod: Arc::new(AtomicBool::new(false)),
                reached_clock_read: Arc::new(AtomicBool::new(false)),
                cancel_after_chmod: None,
                history: history.clone(),
                identity: HostKeyIdentity {
                    algorithm: "ssh-ed25519".into(),
                    fingerprint: "测试替身".into(),
                },
            },
            progress: CaptureProgress(history),
        }
    }

    async fn execute(&self) -> AppResult<()> {
        execute_connected_deployment_observed_with_checkpoint(
            &self.remote,
            &self.plan,
            &self.files,
            &self.config,
            &CancellationToken::new(),
            || async { Ok(()) },
            &self.progress,
        )
        .await
    }

    fn history(&self) -> Vec<Recorded> {
        self.remote.history.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn all_deployment_modes_verify_clock_after_upload_and_before_every_agent_action() {
    for mode in [
        DeploymentMode::FirstDeploy,
        DeploymentMode::FullUpgrade,
        DeploymentMode::ServiceUpgrade,
    ] {
        let fixture = Fixture::new(mode, ClockBehavior::Behind);
        fixture
            .execute()
            .await
            .expect("三类部署均应完成校时和后续执行");
        let history = fixture.history();
        let reads: Vec<_> = history
            .iter()
            .enumerate()
            .filter(|(_, call)| call.is_clock_read())
            .map(|(index, _)| index)
            .collect();
        let write = history
            .iter()
            .position(Recorded::is_clock_write)
            .expect("偏差两分钟必须校时");
        let first_agent = history
            .iter()
            .position(|call| call.agent_action().is_some())
            .unwrap();
        let last_upload = history
            .iter()
            .rposition(|call| matches!(call, Recorded::Upload))
            .unwrap();
        let chmod = history
            .iter()
            .position(
                |call| matches!(call, Recorded::Command { program, .. } if program == "chmod"),
            )
            .unwrap();
        assert_eq!(reads.len(), 2, "校时必须分别读取原时钟和写后时钟");
        assert!(last_upload < chmod && chmod < reads[0]);
        assert!(reads[0] < write && write < reads[1] && reads[1] < first_agent);
        assert!(history[..first_agent].iter().any(|call| matches!(call, Recorded::Progress { stage, code } if stage == "clock_sync" && code == "CLOCK_CHECK_STARTED")), "校时日志必须进入部署任务事件");
        let actual_actions: Vec<_> = history.iter().filter_map(Recorded::agent_action).collect();
        let expected_actions: Vec<_> = fixture
            .plan
            .agent_invocations()
            .into_iter()
            .map(|action| action.action)
            .collect();
        assert_eq!(
            actual_actions, expected_actions,
            "校时不能改变既有部署或回滚动作顺序"
        );
    }
}

#[tokio::test]
async fn aligned_clock_is_observed_without_running_a_clock_write() {
    let fixture = Fixture::new(DeploymentMode::FirstDeploy, ClockBehavior::Aligned);
    fixture.execute().await.unwrap();
    let history = fixture.history();
    assert_eq!(
        history.iter().filter(|call| call.is_clock_read()).count(),
        1
    );
    assert!(!history.iter().any(Recorded::is_clock_write));
    assert!(history.iter().any(|call| call.agent_action().is_some()));
}

#[tokio::test]
async fn rejected_or_unreadable_clock_stops_before_agent_actions() {
    for behavior in [
        ClockBehavior::WriteRejected,
        ClockBehavior::ReadMalformed,
        ClockBehavior::VerificationStaysBehind,
    ] {
        let fixture = Fixture::new(DeploymentMode::FullUpgrade, behavior);
        assert!(fixture.execute().await.is_err());
        assert!(
            !fixture
                .history()
                .iter()
                .any(|call| call.agent_action().is_some()),
            "校时未核实成功时不得进入备份、安装或健康动作"
        );
    }
}

#[tokio::test]
async fn cancellation_before_clock_does_not_change_remote_time_or_start_agent() {
    let cancellation = CancellationToken::new();
    let mut fixture = Fixture::new(DeploymentMode::ServiceUpgrade, ClockBehavior::Behind);
    fixture.remote.cancel_after_chmod = Some(cancellation.clone());
    let result = execute_connected_deployment_observed_with_checkpoint(
        &fixture.remote,
        &fixture.plan,
        &fixture.files,
        &fixture.config,
        &cancellation,
        || async { Ok(()) },
        &fixture.progress,
    )
    .await;
    assert!(matches!(result, Err(AppError::Cancelled)));
    assert!(
        !fixture
            .history()
            .iter()
            .any(|call| call.is_clock_write() || call.agent_action().is_some())
    );
}

#[tokio::test]
async fn lost_checkpoint_before_clock_does_not_change_remote_time_or_start_agent() {
    for fail_after_read in [false, true] {
        let fixture = Fixture::new(DeploymentMode::FullUpgrade, ClockBehavior::Behind);
        let reached_checkpoint = if fail_after_read {
            fixture.remote.reached_clock_read.clone()
        } else {
            fixture.remote.reached_chmod.clone()
        };
        let result = execute_connected_deployment_observed_with_checkpoint(
            &fixture.remote,
            &fixture.plan,
            &fixture.files,
            &fixture.config,
            &CancellationToken::new(),
            || {
                let reached_checkpoint = reached_checkpoint.clone();
                async move {
                    if reached_checkpoint.load(Ordering::SeqCst) {
                        Err(AppError::Conflict("测试模拟校时前租约失效".into()))
                    } else {
                        Ok(())
                    }
                }
            },
            &fixture.progress,
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("租约失效"));
        assert!(
            !fixture
                .history()
                .iter()
                .any(|call| call.is_clock_write() || call.agent_action().is_some())
        );
    }
}
