use super::device::{AdbDevice, DeviceCommandPort, ProcessOutput};
use crate::{
    application::ports::task_event::{TaskEventInput, TaskEventSink},
    core::error::AppResult,
    domain::{
        common::task::{TargetState, TaskEventLevel},
        smart_screen::model::ScreenAsset,
    },
    formal::app_state::FormalAppState,
    infrastructure::local_sqlite::task_repository::TargetUpdate,
};
use std::{
    collections::BTreeMap,
    future::Future,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

/// 设备操作共用阶段进度与日志；具体阶段由安装或配置操作提供。
pub struct ScreenOperationProgress<'a> {
    pub state: &'a FormalAppState,
    pub task: &'a str,
    pub screen: &'a ScreenAsset,
}
pub type InstallationProgress<'a> = ScreenOperationProgress<'a>;
impl ScreenOperationProgress<'_> {
    pub async fn report(
        &self,
        stage: &str,
        percent: u64,
        message: &str,
        persist: bool,
    ) -> AppResult<()> {
        self.state
            .task_repository
            .update_target(
                self.task,
                TargetUpdate {
                    resource_type: "smart_screen".into(),
                    resource_key: self.screen.id.clone(),
                    state: TargetState::Running,
                    stage: stage.into(),
                    progress_current: percent.min(100),
                    progress_total: 100,
                    fencing_token: None,
                    message_code: Some("SCREEN_DEVICE_PROGRESS".into()),
                    message_params_json: Some(serde_json::json!({"summary":message}).to_string()),
                },
            )
            .await?;
        let event = TaskEventInput {
            resource_type: Some("smart_screen".into()),
            resource_key: Some(self.screen.id.clone()),
            stage: stage.into(),
            status: "running".into(),
            progress_current: Some(percent.min(100)),
            progress_total: Some(100),
            level: TaskEventLevel::Info,
            message_code: "SCREEN_DEVICE_PROGRESS".into(),
            message_params: BTreeMap::from([(
                "targetName".into(),
                self.screen.fields.name.clone(),
            )]),
            message: Some(format!(
                "{}（{}）：{message}",
                self.screen.fields.name, self.screen.fields.ip
            )),
        };
        if persist {
            self.state
                .task_event_pipeline
                .emit(self.task, event)
                .await?;
        } else {
            self.state
                .task_event_pipeline
                .emit_transient(self.task, event)
                .await?;
        }
        Ok(())
    }
    pub async fn wait<T>(
        &self,
        stage: &str,
        percent: u64,
        message: &str,
        work: impl Future<Output = AppResult<T>>,
    ) -> AppResult<T> {
        self.report(stage, percent, message, true).await?;
        tokio::pin!(work);
        let start = Instant::now();
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        ticker.tick().await;
        let mut last_log = 0;
        loop {
            tokio::select! {
                result=&mut work=>return result,
                _=ticker.tick()=>{let elapsed=start.elapsed().as_secs();let persist=elapsed>=last_log+10;
                    if let Err(error)=self.report(stage,percent,&format!("{message}；已等待 {elapsed} 秒"),persist).await{tracing::warn!(error=?crate::core::log_safety::safe_error(&error),"installation progress reporting failed");}
                    if persist{last_log=elapsed;}
                }
            }
        }
    }
    pub async fn upload<P: DeviceCommandPort>(
        &self,
        device: &AdbDevice<P>,
        path: &str,
        remote: &str,
        size: u64,
        cancel: CancellationToken,
    ) -> AppResult<ProcessOutput> {
        self.report(
            "传输安装包",
            20,
            &format!(
                "开始传输安装包，共 {:.1} MB；传输完成后才开始安装",
                mb(size)
            ),
            true,
        )
        .await?;
        let source = upload_source(path)?;
        let args = ["push", &source, remote];
        let work = device.adb(
            &self.screen.fields.ip,
            &args,
            Duration::from_secs(600),
            cancel.clone(),
        );
        tokio::pin!(work);
        let start = Instant::now();
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        ticker.tick().await;
        let (mut sent, mut last_log_percent, mut last_log_seconds) = (0u64, 0u64, 0u64);
        loop {
            tokio::select! {
                result=&mut work=>{
                    if result.as_ref().is_ok_and(|output|output.success){self.report("传输完成",40,&format!("安装包已传输完成：{:.1} MB，用时 {} 秒",mb(size),start.elapsed().as_secs()),true).await?;}
                    return result;
                }
                _=ticker.tick()=>{
                    let probe=device.adb(&self.screen.fields.ip,&["shell","stat","-c","%s",remote],Duration::from_secs(2),cancel.clone()).await;
                    if let Ok(output)=probe{if output.success{if let Ok(bytes)=output.stdout.trim().parse::<u64>(){sent=sent.max(bytes.min(size));}}}
                    let percentage=transfer_percent(sent,size);let elapsed=start.elapsed().as_secs();
                    let persist=percentage>=last_log_percent+10||elapsed>=last_log_seconds+10;
                    self.report("传输安装包",20+percentage/5,&format!("正在传输安装包：{:.1} / {:.1} MB（{percentage}%），已用 {elapsed} 秒",mb(sent),mb(size)),persist).await?;
                    if persist{last_log_percent=percentage;last_log_seconds=elapsed;}
                }
            }
        }
    }
}
fn upload_source(path: &str) -> AppResult<String> {
    #[cfg(windows)]
    {
        // ADB 的 Windows 文件读取需要扩展路径，普通路径超过 MAX_PATH 会被误报为不存在。
        std::fs::canonicalize(path)
            .map(|absolute| absolute.to_string_lossy().into_owned())
            .map_err(|error| crate::core::error::AppError::io("读取待传输安装包路径", &error))
    }
    #[cfg(not(windows))]
    {
        Ok(path.to_string())
    }
}
fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}
pub fn transfer_percent(sent: u64, total: u64) -> u64 {
    if total == 0 {
        0
    } else {
        ((u128::from(sent.min(total)) * 100) / u128::from(total)) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn adb_upload_retains_extended_windows_path_for_long_task_directories() {
        let temp = tempfile::tempdir().unwrap();
        let mut directory = temp.path().join("中文安装包");
        for _ in 0..5 { directory = directory.join("screen-task-long-directory-0123456789abcdef"); }
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("xiaoxin.apk");
        std::fs::write(&file, b"test-apk").unwrap();
        assert!(file.to_string_lossy().len() > 260);
        let argument = upload_source(&file.to_string_lossy()).unwrap();
        assert!(argument.starts_with(r"\\?\"));
        assert_eq!(std::fs::read(argument).unwrap(), b"test-apk");
    }
    #[test]
    fn transfer_progress_only_uses_reported_bytes() {
        assert_eq!(transfer_percent(0, 100), 0);
        assert_eq!(transfer_percent(47, 100), 47);
        assert_eq!(transfer_percent(110, 100), 100);
        assert_eq!(transfer_percent(10, 0), 0);
        assert_eq!(transfer_percent(u64::MAX, u64::MAX), 100);
    }
}
