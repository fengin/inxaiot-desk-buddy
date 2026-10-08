use super::{
    device::{AdbDevice, AndroidTools},
    task_data,
};
use crate::{
    core::error::{AppError, AppResult},
    domain::smart_screen::{
        model::{ScreenAsset, ScreenObservation},
        operation::ScreenPlan,
    },
    formal::app_state::FormalAppState,
    infrastructure::project_context::map_formal_error,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;
fn path(state: &FormalAppState, project: &str, task: &str, screen: &str) -> AppResult<PathBuf> {
    Ok(state
        .paths
        .project_task_dir(project, task)
        .map_err(map_formal_error)?
        .join(format!(
            "diagnostic-{}.json",
            &hex::encode(Sha256::digest(screen.as_bytes()))[..16]
        )))
}
pub fn bounded_log(text: &str) -> String {
    let sensitive = regex::Regex::new(
        r"(?i)token|password|secret|authorization|auth_code|https?://|wss?://|extras=|intent \{",
    )
    .unwrap();
    text.lines()
        .filter(|line| !sensitive.is_match(line))
        .take(200)
        .map(|line| line.chars().take(1000).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
pub async fn collect(
    state: &FormalAppState,
    plan: &ScreenPlan,
    task: &str,
    screen: &ScreenAsset,
    observed: &ScreenObservation,
) -> AppResult<Value> {
    let device = AdbDevice::new(AndroidTools::discover()?);
    let mut errors = Vec::new();
    let mut readings = serde_json::Map::new();
    if observed.adb_available {
        for (name, args) in [
            ("bootId", vec!["cat", "/proc/sys/kernel/random/boot_id"]),
            ("uptime", vec!["cat", "/proc/uptime"]),
            ("systemReady", vec!["getprop", "sys.boot_completed"]),
            ("runningAdbPort", vec!["getprop", "service.adb.tcp.port"]),
            ("timezone", vec!["getprop", "persist.sys.timezone"]),
            (
                "automaticTime",
                vec!["settings", "get", "global", "auto_time"],
            ),
        ] {
            match device
                .shell(&screen.fields.ip, &args, CancellationToken::new())
                .await
            {
                Ok(value) => {
                    readings.insert(
                        name.into(),
                        json!(value.chars().take(2048).collect::<String>()),
                    );
                }
                Err(error) => errors.push(format!("{name}：{error}")),
            }
        }
        match device
            .shell(
                &screen.fields.ip,
                &[
                    "logcat",
                    "-d",
                    "-t",
                    "200",
                    "-v",
                    "threadtime",
                    "ActivityManager:I",
                    "PackageManager:I",
                    "AndroidRuntime:E",
                    "*:S",
                ],
                CancellationToken::new(),
            )
            .await
        {
            Ok(text) => {
                readings.insert(
                    "recentSystemLog".into(),
                    json!(state.task_event_pipeline.redact_text(&bounded_log(&text))),
                );
            }
            Err(error) => errors.push(format!("系统日志：{error}")),
        }
    }
    let document = json!({"formatVersion":1,"title":"智能屏诊断记录","taskId":task,"localProjectId":plan.project_id,"sourceComputer":plan.instance_id,"operator":plan.operator,"screen":{"id":screen.id,"name":screen.fields.name,"ip":screen.fields.ip,"size":screen.fields.size,"source":screen.source},"observedAt":observed.observed_at,"device":observed,"system":readings,"collectionErrors":errors,"scope":"设备和应用检查、最近200行指定系统日志；过滤凭据和网络地址相关日志，不读取应用私有配置。完整文件只保存在本机。"});
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|_| AppError::InvalidConfig("诊断内容转换失败".into()))?;
    if bytes.len() > 512 * 1024 {
        return Err(AppError::Conflict("诊断内容超过单台512KB限制".into()));
    }
    let target = path(state, &plan.project_id, task, &screen.id)?;
    tokio::fs::create_dir_all(target.parent().unwrap())
        .await
        .map_err(|e| AppError::io("创建诊断目录", &e))?;
    tokio::fs::write(&target, &bytes)
        .await
        .map_err(|e| AppError::io("保存诊断文件", &e))?;
    Ok(
        json!({"sha256":hex::encode(Sha256::digest(&bytes)),"bytes":bytes.len(),"collectionErrors":errors}),
    )
}
pub async fn read(state: &FormalAppState, project: &str, task: &str) -> AppResult<String> {
    let record = state.task_repository.get(task).await?;
    if record.local_project_id != project
        || record.domain_type != "smart_screen"
        || record.operation_type != "diagnostics"
    {
        return Err(AppError::Conflict("诊断记录不属于当前项目和操作".into()));
    }
    if !record.state.is_terminal()
        && record.state != crate::domain::common::task::TaskState::FinalizingFailed
    {
        return Err(AppError::Conflict("请等待本次诊断采集完成".into()));
    }
    let (plan, _) = task_data::read_plan(state.local_store.pool(), project, task).await?;
    let results = task_data::read_results(state.local_store.pool(), project, task).await?;
    let mut documents = Vec::new();
    let mut missing = Vec::new();
    let mut total = 0;
    for screen in &plan.targets {
        let file = path(state, project, task, &screen.id)?;
        let Some(expected) = results
            .targets
            .get(&screen.id)
            .and_then(|r| r.evidence.get("diagnostic"))
            .and_then(|r| r.get("sha256"))
            .and_then(Value::as_str)
        else {
            missing.push(screen.fields.name.clone());
            continue;
        };
        match tokio::fs::read(&file).await {
            Err(_) => missing.push(screen.fields.name.clone()),
            Ok(bytes) => {
                total += bytes.len();
                if total > 20 * 1024 * 1024 {
                    return Err(AppError::Conflict(
                        "所选诊断总量超过20MB，请按较小批次采集".into(),
                    ));
                }
                if hex::encode(Sha256::digest(&bytes)) != expected {
                    return Err(AppError::Integrity {
                        operation: "诊断文件完整性核对",
                    });
                }
                documents.push(
                    serde_json::from_slice::<Value>(&bytes)
                        .map_err(|_| AppError::InvalidConfig("诊断文件格式异常".into()))?,
                );
            }
        }
    }
    if documents.is_empty() {
        return Err(AppError::NotFound(
            "本机诊断文件不存在或已清理，请重新采集".into(),
        ));
    }
    serde_json::to_string_pretty(&json!({"title":"智能屏诊断报告","taskId":task,"sourceComputer":plan.instance_id,"missingScreens":missing,"screens":documents})).map_err(|_|AppError::InvalidConfig("整理诊断报告失败".into()))
}
pub async fn export(
    state: &FormalAppState,
    project: &str,
    task: &str,
    directory: &Path,
) -> AppResult<String> {
    let contents = read(state, project, task).await?;
    let directory = tokio::fs::canonicalize(directory)
        .await
        .map_err(|e| AppError::io("读取导出目录", &e))?;
    if !directory.is_dir() {
        return Err(AppError::InvalidConfig("请选择导出文件夹".into()));
    }
    let output = directory.join(format!("智能屏诊断-{}.json", uuid::Uuid::now_v7()));
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&output)
        .await
        .map_err(|e| AppError::io("创建诊断报告", &e))?;
    file.write_all(contents.as_bytes())
        .await
        .map_err(|e| AppError::io("导出诊断报告", &e))?;
    Ok(output
        .to_string_lossy()
        .strip_prefix(r"\\?\")
        .unwrap_or(&output.to_string_lossy())
        .into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_credentials_urls_and_limits_logs() {
        let logs = "system ready\ntoken=private\nhttps://example.invalid/?a=b\nnormal event";
        assert_eq!(bounded_log(logs), "system ready\nnormal event");
        assert_eq!(
            bounded_log(&vec!["ok"; 250].join("\n")).lines().count(),
            200
        );
    }
}
