use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::release::validate_release_version;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentMode {
    FirstDeploy,
    FullUpgrade,
    ServiceUpgrade,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentImageInput {
    pub service_name: String,
    pub file_path: String,
    pub image_tag: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentPlanInput {
    pub mode: DeploymentMode,
    pub target_macs: Vec<String>,
    #[serde(default)]
    pub image_files: Vec<DeploymentImageInput>,
    #[serde(default)]
    pub artifact_path: String,
    #[serde(default)]
    pub artifact_name: String,
    #[serde(default)]
    pub artifact_version: String,
    #[serde(default)]
    pub service_name: Option<String>,
    #[serde(default)]
    pub image_name: Option<String>,
    #[serde(default)]
    pub service_image_environment_variable: Option<String>,
    #[serde(default)]
    pub images: BTreeMap<String, String>,
    pub batch_size: u32,
    pub concurrency: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentStep {
    pub code: String,
    pub label: String,
    pub agent_action: Option<String>,
    pub weight: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentPlan {
    pub mode: DeploymentMode,
    pub target_macs: Vec<String>,
    pub image_files: Vec<DeploymentImageInput>,
    pub artifact_path: String,
    pub artifact_name: String,
    pub artifact_version: String,
    pub service_name: Option<String>,
    pub image_name: Option<String>,
    pub service_image_environment_variable: Option<String>,
    pub images: BTreeMap<String, String>,
    pub batch_size: u32,
    pub concurrency: u32,
    pub steps: Vec<DeploymentStep>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentInvocation {
    pub action: String,
    pub environment: BTreeMap<String, String>,
}

impl DeploymentPlan {
    pub fn build(mut input: DeploymentPlanInput) -> AppResult<Self> {
        input.target_macs.sort();
        input.target_macs.dedup();
        if input.target_macs.is_empty() {
            return Err(AppError::InvalidConfig("部署目标不能为空".into()));
        }
        if input.image_files.is_empty() {
            return Err(AppError::InvalidConfig("请至少选择一个服务镜像".into()));
        }
        let service_pattern =
            regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]*$").expect("static service name regex");
        let mut selected_images = BTreeMap::new();
        for image in &mut input.image_files {
            image.service_name = image.service_name.trim().to_string();
            image.file_path = image.file_path.trim().to_string();
            image.image_tag = image.image_tag.trim().to_string();
            if !service_pattern.is_match(&image.service_name) {
                return Err(AppError::InvalidConfig(format!(
                    "服务名无效：{}",
                    image.service_name
                )));
            }
            if image.file_path.is_empty() {
                return Err(AppError::InvalidConfig(format!(
                    "服务 {} 尚未选择镜像文件",
                    image.service_name
                )));
            }
            if image.image_tag.is_empty()
                || image.image_tag.chars().any(char::is_whitespace)
                || image.image_tag.chars().any(char::is_control)
            {
                return Err(AppError::InvalidConfig(format!(
                    "服务 {} 的镜像标签无效",
                    image.service_name
                )));
            }
            if selected_images
                .insert(image.service_name.clone(), image.image_tag.clone())
                .is_some()
            {
                return Err(AppError::InvalidConfig(format!(
                    "服务 {} 重复选择了镜像",
                    image.service_name
                )));
            }
        }
        input.images = selected_images;
        if input.artifact_path.trim().is_empty() {
            input.artifact_path = input.image_files[0].file_path.clone();
        }
        if input.artifact_name.trim().is_empty() {
            input.artifact_name = if input.mode == DeploymentMode::ServiceUpgrade {
                input.image_files[0].service_name.clone()
            } else {
                "镜像组合".into()
            };
        }
        if input.artifact_version.trim().is_empty() {
            input.artifact_version = if input.mode == DeploymentMode::ServiceUpgrade {
                input.image_files[0]
                    .image_tag
                    .rsplit_once(':')
                    .map(|(_, version)| version.to_string())
                    .unwrap_or_else(|| "latest".into())
            } else {
                "generated".into()
            };
        }
        validate_release_version(&input.artifact_version)?;
        validate_deployment_batch(input.batch_size, input.concurrency, input.target_macs.len())?;
        if input.mode == DeploymentMode::ServiceUpgrade {
            if input.image_files.len() != 1 {
                return Err(AppError::InvalidConfig(
                    "单服升级只能选择一个服务镜像".into(),
                ));
            }
            input.service_name = Some(input.image_files[0].service_name.clone());
            input.image_name = Some(input.image_files[0].image_tag.clone());
        } else {
            input.service_name = None;
            input.image_name = None;
        }
        let steps = match input.mode {
            DeploymentMode::FirstDeploy => vec![
                step("precheck", "远端环境预检", Some("precheck"), 10),
                step("upload", "上传镜像与配置", None, 30),
                step("install", "安装并启动全部服务", Some("install"), 35),
                step("register", "调用一体机本地注册接口", None, 10),
                step("health", "服务与版本检查", Some("health"), 15),
            ],
            DeploymentMode::FullUpgrade => vec![
                step("precheck", "远端环境预检", Some("precheck"), 10),
                step("backup", "备份当前配置与必要数据", Some("backup"), 15),
                step("upload", "上传镜像与配置", None, 25),
                step("install", "升级并重建全部服务", Some("install"), 35),
                step("health", "服务与版本检查", Some("health"), 15),
            ],
            DeploymentMode::ServiceUpgrade => vec![
                step("service_check", "检查目标服务", Some("service-check"), 15),
                step("upload", "上传单个镜像", None, 35),
                step(
                    "service_upgrade",
                    "加载镜像并重建目标服务",
                    Some("service-upgrade"),
                    35,
                ),
                step(
                    "service_health",
                    "目标服务健康检查",
                    Some("service-health"),
                    15,
                ),
            ],
        };
        Ok(Self {
            mode: input.mode,
            target_macs: input.target_macs,
            image_files: input.image_files,
            artifact_path: input.artifact_path,
            artifact_name: input.artifact_name,
            artifact_version: input.artifact_version,
            service_name: input.service_name,
            image_name: input.image_name,
            service_image_environment_variable: input.service_image_environment_variable,
            images: input.images,
            batch_size: input.batch_size,
            concurrency: input.concurrency,
            steps,
        })
    }

    pub fn agent_invocations(&self) -> Vec<AgentInvocation> {
        self.steps
            .iter()
            .filter_map(|step| {
                let action = step.agent_action.clone()?;
                let mut environment = BTreeMap::new();
                environment.insert("RELEASE_VERSION".into(), self.artifact_version.clone());
                if let Some(service) = &self.service_name {
                    environment.insert("SERVICE_NAME".into(), service.clone());
                }
                if let Some(image) = &self.image_name {
                    environment.insert("SERVICE_IMAGE".into(), image.clone());
                }
                if let Some(variable) = &self.service_image_environment_variable {
                    environment.insert("SERVICE_IMAGE_ENV".into(), variable.clone());
                }
                Some(AgentInvocation {
                    action,
                    environment,
                })
            })
            .collect()
    }
}

/// 校验批次执行参数。预检按检查事项展示进度时也复用这一份领域规则，
/// 避免页面预检与最终计划构建出现两套口径。
pub fn validate_deployment_batch(
    batch_size: u32,
    concurrency: u32,
    target_count: usize,
) -> AppResult<()> {
    if batch_size == 0 || concurrency == 0 || concurrency > batch_size {
        return Err(AppError::InvalidConfig(
            "批次和并发必须大于0，且并发不能超过批次".into(),
        ));
    }
    if batch_size > 20 || concurrency > 5 {
        return Err(AppError::InvalidConfig(
            "一批台数不能超过20，并发数不能超过5".into(),
        ));
    }
    if batch_size as usize > target_count {
        return Err(AppError::InvalidConfig(
            "一批台数不能大于部署目标数量".into(),
        ));
    }
    Ok(())
}

fn step(code: &str, label: &str, action: Option<&str>, weight: u32) -> DeploymentStep {
    DeploymentStep {
        code: code.into(),
        label: label.into(),
        agent_action: action.map(str::to_string),
        weight,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::core::error::AppError;

    use super::{DeploymentMode, DeploymentPlan, DeploymentPlanInput};

    fn input(mode: DeploymentMode) -> DeploymentPlanInput {
        DeploymentPlanInput {
            mode,
            target_macs: vec![
                "001122334451".into(),
                "001122334452".into(),
                "001122334453".into(),
                "001122334454".into(),
            ],
            image_files: vec![super::DeploymentImageInput {
                service_name: "device-edge".into(),
                file_path: "C:/device-edge.tar".into(),
                image_tag: "device-edge:1".into(),
            }],
            artifact_path: "C:/release".into(),
            artifact_name: "Release".into(),
            artifact_version: "2026.08.28".into(),
            service_name: None,
            image_name: None,
            service_image_environment_variable: None,
            images: BTreeMap::new(),
            batch_size: 4,
            concurrency: 2,
        }
    }

    #[test]
    fn three_modes_have_fixed_agent_actions_and_weights() {
        for mode in [
            DeploymentMode::FirstDeploy,
            DeploymentMode::FullUpgrade,
            DeploymentMode::ServiceUpgrade,
        ] {
            let mut input = input(mode);
            if mode == DeploymentMode::ServiceUpgrade {
                input.service_name = Some("device-edge".into());
                input.image_name = Some("device-edge:1".into());
            }
            let plan = DeploymentPlan::build(input).expect("plan");
            assert_eq!(plan.steps.iter().map(|step| step.weight).sum::<u32>(), 100);
            for invocation in plan.agent_invocations() {
                assert!(matches!(
                    invocation.action.as_str(),
                    "precheck"
                        | "backup"
                        | "install"
                        | "health"
                        | "service-check"
                        | "service-upgrade"
                        | "service-health"
                ));
            }
        }
    }

    #[test]
    fn batch_size_and_concurrency_are_bounded_by_target_count() {
        let mut invalid_batch = input(DeploymentMode::FullUpgrade);
        invalid_batch.target_macs.truncate(2);
        invalid_batch.batch_size = 3;
        invalid_batch.concurrency = 2;
        assert!(matches!(
            DeploymentPlan::build(invalid_batch),
            Err(AppError::InvalidConfig(message)) if message.contains("不能大于部署目标数量")
        ));

        let mut valid = input(DeploymentMode::FullUpgrade);
        valid.target_macs.truncate(2);
        valid.batch_size = 2;
        valid.concurrency = 2;
        assert!(DeploymentPlan::build(valid).is_ok());

        let mut over_system_limit = input(DeploymentMode::FullUpgrade);
        over_system_limit.target_macs =
            (0..21).map(|index| format!("00112233{index:04}")).collect();
        over_system_limit.batch_size = 21;
        over_system_limit.concurrency = 2;
        assert!(matches!(
            DeploymentPlan::build(over_system_limit),
            Err(AppError::InvalidConfig(message)) if message.contains("不能超过20")
        ));
    }

    #[test]
    fn service_upgrade_rejects_arbitrary_service_and_invalid_concurrency() {
        let mut invalid = input(DeploymentMode::ServiceUpgrade);
        invalid.image_files[0].service_name = "$(bad)".into();
        assert!(DeploymentPlan::build(invalid).is_err());

        let mut invalid = input(DeploymentMode::FullUpgrade);
        invalid.concurrency = 5;
        invalid.batch_size = 2;
        assert!(DeploymentPlan::build(invalid).is_err());

        let mut traversal = input(DeploymentMode::FullUpgrade);
        traversal.artifact_version = "../../outside".into();
        assert!(DeploymentPlan::build(traversal).is_err());
    }
}
