use crate::core::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::UNIX_EPOCH;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApkInfo {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub last_modified: u64,
    pub sha256: String,
    pub package_id: String,
    pub app_version: String,
    pub app_version_code: u64,
    pub min_sdk: u32,
    pub abis: Vec<String>,
    pub activity: String,
}
fn executable_arg(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(value) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{value}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
    }
}
pub async fn sha256(path: &Path) -> AppResult<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| AppError::io("打开 APK 文件", &e))?;
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut hash = Sha256::new();
    loop {
        let count = file
            .read(&mut buffer)
            .await
            .map_err(|e| AppError::io("读取 APK 文件", &e))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex::encode(hash.finalize()))
}
pub async fn inspect(path: &Path, cancel: CancellationToken) -> AppResult<ApkInfo> {
    if !path
        .extension()
        .is_some_and(|v| v.eq_ignore_ascii_case("apk"))
    {
        return Err(AppError::InvalidConfig("请选择 APK 文件".into()));
    }
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let canonical = tokio::fs::canonicalize(path)
        .await
        .map_err(|e| AppError::io("读取安装包路径", &e))?;
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|e| AppError::io("读取安装包信息", &e))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 2 * 1024 * 1024 * 1024 {
        return Err(AppError::InvalidConfig(
            "安装包必须是非空、2 GB 以内的文件".into(),
        ));
    }
    let hash = sha256(&canonical).await?;
    let parsing_path = canonical.clone();
    let token = cancel.clone();
    let info =
        tokio::task::spawn_blocking(move || super::apk_manifest::read(&parsing_path, &token))
            .await
            .map_err(|_| AppError::InvalidConfig("安装包信息解析未完成，请重新选择".into()))??;
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    if sha256(&canonical).await? != hash {
        return Err(AppError::Conflict(
            "解析期间安装包发生变化，请重新选择".into(),
        ));
    }
    Ok(ApkInfo {
        name: canonical
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        path: executable_arg(&canonical),
        size: metadata.len(),
        last_modified: metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        sha256: hash,
        package_id: info.package,
        app_version: info.version,
        app_version_code: info.code,
        min_sdk: info.sdk,
        abis: info.abis,
        activity: info.activity,
    })
}

pub async fn stage(info: &ApkInfo, directory: &Path) -> AppResult<ApkInfo> {
    if sha256(Path::new(&info.path)).await? != info.sha256 {
        return Err(AppError::Conflict("安装包已改变，请重新选择并检查".into()));
    }
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|e| AppError::io("创建安装任务目录", &e))?;
    let destination = directory.join("xiaoxin.apk");
    tokio::fs::copy(&info.path, &destination)
        .await
        .map_err(|e| AppError::io("准备本次安装包", &e))?;
    if sha256(&destination).await? != info.sha256 {
        return Err(AppError::Integrity {
            operation: "本次安装包复制校验",
        });
    }
    let mut staged = info.clone();
    staged.path = executable_arg(&destination);
    Ok(staged)
}
