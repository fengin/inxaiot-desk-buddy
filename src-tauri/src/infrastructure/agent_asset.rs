use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::core::error::{AppError, AppResult};
use crate::formal::release_profile_repository::StoredReleaseAgentScript;

pub const AGENT_SOURCE: &str = include_str!("../../resources/agent/edge-node-agent.sh");
pub const AGENT_FILE_NAME: &str = "edge-node-agent.sh";
pub const AGENT_VERSION: &str = "0.1.13";
pub const AGENT_PROTOCOL_VERSION: &str = "1";
pub const AGENT_SHA256: &str = "fb7ad58c72dbbbbc3660401ecec38a840965b5596bb78929c42a64f0c7fe377c";
pub const MAX_AGENT_SCRIPT_BYTES: usize = 1024 * 1024;
pub const AGENT_COMPATIBILITY: &[&str] = &[
    "Linux x86_64",
    "Docker Engine 20.10+",
    "docker-compose v1 或 docker compose v2",
    "POSIX sh、tar、awk",
];

const REQUIRED_ACTIONS: &[&str] = &[
    "version",
    "precheck",
    "install",
    "backup",
    "health",
    "service-check",
    "service-upgrade",
    "service-health",
    "inspect-services",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentScriptAsset {
    pub content: String,
    pub version: String,
    pub protocol_version: String,
    pub sha256: String,
}

pub fn inspect_agent_source(source: &str) -> AppResult<AgentScriptAsset> {
    if source.is_empty() || source.len() > MAX_AGENT_SCRIPT_BYTES || source.contains('\0') {
        return Err(AppError::InvalidConfig(
            "一体机脚本必须是1MiB以内的有效文本文件".into(),
        ));
    }
    let first_line = source.lines().next().unwrap_or_default().trim();
    if !matches!(first_line, "#!/usr/bin/env sh" | "#!/bin/sh") {
        return Err(AppError::InvalidConfig(
            "一体机脚本必须使用POSIX sh解释器".into(),
        ));
    }
    let version = assignment(source, "AGENT_VERSION")
        .ok_or_else(|| AppError::InvalidConfig("一体机脚本缺少AGENT_VERSION".into()))?;
    if version.is_empty()
        || version.len() > 64
        || !version
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'.' | b'-' | b'_'))
    {
        return Err(AppError::InvalidConfig(
            "一体机脚本的AGENT_VERSION格式无效".into(),
        ));
    }
    let protocol_version = assignment(source, "AGENT_PROTOCOL_VERSION")
        .ok_or_else(|| AppError::InvalidConfig("一体机脚本缺少AGENT_PROTOCOL_VERSION".into()))?;
    if protocol_version != AGENT_PROTOCOL_VERSION {
        return Err(AppError::InvalidConfig(format!(
            "一体机脚本协议版本不兼容：需要{AGENT_PROTOCOL_VERSION}，实际为{protocol_version}"
        )));
    }
    let missing_actions = REQUIRED_ACTIONS
        .iter()
        .filter(|action| {
            !source.lines().any(|line| {
                line.trim_start()
                    .strip_prefix(**action)
                    .is_some_and(|tail| tail.trim_start().starts_with(')'))
            })
        })
        .copied()
        .collect::<Vec<_>>();
    if !missing_actions.is_empty() {
        return Err(AppError::InvalidConfig(format!(
            "一体机脚本缺少必要动作：{}",
            missing_actions.join("、")
        )));
    }
    Ok(AgentScriptAsset {
        content: source.into(),
        version,
        protocol_version,
        sha256: hex::encode(Sha256::digest(source.as_bytes())),
    })
}

pub fn load_agent_script_file(path: &Path) -> AppResult<AgentScriptAsset> {
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case("sh"))
    {
        return Err(AppError::InvalidConfig(
            "请选择扩展名为.sh的一体机脚本".into(),
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| AppError::io("读取一体机脚本", &error))?;
    if bytes.len() > MAX_AGENT_SCRIPT_BYTES {
        return Err(AppError::InvalidConfig("一体机脚本不能超过1MiB".into()));
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| AppError::InvalidConfig("一体机脚本必须使用UTF-8编码".into()))?;
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    inspect_agent_source(&normalized)
}

pub fn embedded_agent_asset() -> AppResult<AgentScriptAsset> {
    inspect_agent_source(AGENT_SOURCE)
}

pub fn effective_agent_asset(
    custom: Option<&StoredReleaseAgentScript>,
) -> AppResult<AgentScriptAsset> {
    let Some(custom) = custom else {
        return embedded_agent_asset();
    };
    let asset = inspect_agent_source(&custom.content)?;
    if asset.version != custom.version
        || asset.protocol_version != custom.protocol_version
        || asset.sha256 != custom.sha256
    {
        return Err(AppError::Integrity {
            operation: "校验项目一体机脚本",
        });
    }
    Ok(asset)
}

pub fn materialize_agent_for_view(
    runtime_agent_dir: &Path,
    asset: &AgentScriptAsset,
) -> AppResult<PathBuf> {
    let directory = runtime_agent_dir.join(&asset.sha256);
    std::fs::create_dir_all(&directory)
        .map_err(|error| AppError::io("创建一体机脚本查看目录", &error))?;
    let path = directory.join(AGENT_FILE_NAME);
    std::fs::write(&path, &asset.content)
        .map_err(|error| AppError::io("写入一体机脚本查看文件", &error))?;
    resolve_agent_view_file(&path)
}

pub fn open_agent_in_system_editor(path: &Path) -> AppResult<()> {
    let path = resolve_agent_view_file(path)?;
    #[cfg(target_os = "windows")]
    let mut command = Command::new(windows_notepad_executable()?);
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg("-e");
        command
    };
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");
    command
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| AppError::io("使用系统文本编辑器打开一体机脚本", &error))
}

fn resolve_agent_view_file(path: &Path) -> AppResult<PathBuf> {
    let path = std::fs::canonicalize(path)
        .map_err(|error| AppError::io("定位一体机脚本查看文件", &error))?;
    if !path.is_file() {
        return Err(AppError::InvalidConfig("一体机脚本查看文件无效".into()));
    }
    Ok(path)
}

#[cfg(target_os = "windows")]
fn windows_notepad_executable() -> AppResult<PathBuf> {
    let system_root = std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let executable = system_root.join("System32").join("notepad.exe");
    if executable.is_file() {
        Ok(executable)
    } else {
        Err(AppError::InvalidConfig(
            "无法找到系统记事本，请检查Windows系统组件".into(),
        ))
    }
}

fn assignment(source: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}=\"");
    let values = source
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix(&prefix)
                .and_then(|value| value.strip_suffix('"'))
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    (values.len() == 1).then(|| values[0].clone())
}

pub fn verify_embedded_agent() -> AppResult<()> {
    let asset = embedded_agent_asset()?;
    if asset.sha256 != AGENT_SHA256 {
        return Err(AppError::Integrity {
            operation: "校验内嵌Agent哈希",
        });
    }
    if asset.version != AGENT_VERSION || asset.protocol_version != AGENT_PROTOCOL_VERSION {
        return Err(AppError::Integrity {
            operation: "校验内嵌Agent版本契约",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AGENT_COMPATIBILITY, AGENT_PROTOCOL_VERSION, AGENT_SHA256, AGENT_SOURCE, AGENT_VERSION,
        inspect_agent_source, load_agent_script_file, materialize_agent_for_view,
        resolve_agent_view_file, verify_embedded_agent,
    };

    #[test]
    fn embedded_agent_has_version_hash_protocol_and_compatibility() {
        verify_embedded_agent().expect("embedded agent");
        assert_eq!(AGENT_VERSION, "0.1.13");
        assert_eq!(AGENT_PROTOCOL_VERSION, "1");
        assert_eq!(AGENT_SHA256.len(), 64);
        assert!(!AGENT_COMPATIBILITY.is_empty());
        assert!(AGENT_SOURCE.contains("require_safe_release_version"));
        assert!(AGENT_SOURCE.contains("BACKUP_RETENTION_DAYS=\"${BACKUP_RETENTION_DAYS:-30}\""));
        assert!(
            AGENT_SOURCE.contains(
                "SERVICE_UPGRADE_RETENTION_DAYS=\"${SERVICE_UPGRADE_RETENTION_DAYS:-14}\""
            )
        );
        assert!(AGENT_SOURCE.contains("STAGING_RETENTION_DAYS=\"${STAGING_RETENTION_DAYS:-3}\""));
        assert!(AGENT_SOURCE.contains("stopping current release for consistent database backup"));
        assert!(AGENT_SOURCE.contains("target_file=\"$backup_dir/data/$relative_path\""));
        assert!(AGENT_SOURCE.contains("REMOTE_COMPOSE"));
        assert!(
            AGENT_SOURCE.contains("cp \"$REMOTE_COMPOSE\" \"$new_release_dir/docker-compose.yml\"")
        );
    }

    #[test]
    fn custom_agent_requires_the_current_protocol_and_all_actions() {
        let valid = inspect_agent_source(AGENT_SOURCE).expect("valid agent");
        assert_eq!(valid.protocol_version, "1");
        let incompatible = AGENT_SOURCE.replace(
            "AGENT_PROTOCOL_VERSION=\"1\"",
            "AGENT_PROTOCOL_VERSION=\"2\"",
        );
        assert!(
            inspect_agent_source(&incompatible)
                .expect_err("protocol mismatch")
                .to_string()
                .contains("协议版本不兼容")
        );
        let missing_action = AGENT_SOURCE.replace("  service-health)", "  removed)");
        assert!(
            inspect_agent_source(&missing_action)
                .expect_err("missing action")
                .to_string()
                .contains("service-health")
        );
        let missing_inspection = AGENT_SOURCE.replace("  inspect-services)", "  removed)");
        assert!(inspect_agent_source(&missing_inspection).is_err());
    }

    #[test]
    fn custom_agent_file_is_utf8_and_normalized_to_lf() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("custom-agent.sh");
        std::fs::write(&path, AGENT_SOURCE.replace('\n', "\r\n")).expect("script");
        let loaded = load_agent_script_file(&path).expect("load custom agent");
        assert!(!loaded.content.contains('\r'));
        assert_eq!(loaded.version, AGENT_VERSION);
    }

    #[test]
    fn effective_agent_is_materialized_as_a_canonical_file_for_system_viewing() {
        let root = tempfile::tempdir().expect("tempdir");
        let asset = inspect_agent_source(AGENT_SOURCE).expect("agent");
        let path = materialize_agent_for_view(root.path(), &asset).expect("materialize");
        assert_eq!(
            path,
            std::fs::canonicalize(root.path().join(&asset.sha256).join("edge-node-agent.sh"))
                .expect("canonical view file")
        );
        assert!(path.is_absolute());
        assert_eq!(
            std::fs::read_to_string(path).expect("content"),
            AGENT_SOURCE
        );
    }

    #[test]
    fn system_viewing_rejects_a_missing_file_instead_of_passing_it_to_the_editor() {
        let root = tempfile::tempdir().expect("tempdir");
        let error = resolve_agent_view_file(&root.path().join("missing-agent.sh"))
            .expect_err("missing file must not be opened");
        assert!(error.to_string().contains("定位一体机脚本查看文件"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_viewing_uses_the_explicit_system_notepad() {
        let executable = super::windows_notepad_executable().expect("system notepad");
        assert!(executable.is_file());
        assert_eq!(
            executable
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_ascii_lowercase),
            Some("notepad.exe".into())
        );
    }
}
