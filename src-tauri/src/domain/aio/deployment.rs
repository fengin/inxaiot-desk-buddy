use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::release::validate_release_version;

const SERVICE_WHITELIST: &[&str] = &["emqx", "device-edge", "rule-engine", "device-edge-web"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentMode {
    FirstDeploy,
    FullUpgrade,
    ServiceUpgrade,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentPlanInput {
    pub mode: DeploymentMode,
    pub target_macs: Vec<String>,
    pub artifact_path: String,
    pub artifact_name: String,
    pub artifact_version: String,
    pub service_name: Option<String>,
    pub image_name: Option<String>,
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
    pub artifact_path: String,
    pub artifact_name: String,
    pub artifact_version: String,
    pub service_name: Option<String>,
    pub image_name: Option<String>,
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
        if input.artifact_path.trim().is_empty()
            || input.artifact_name.trim().is_empty()
            || input.artifact_version.trim().is_empty()
        {
            return Err(AppError::InvalidConfig("部署发布物信息不完整".into()));
        }
        validate_release_version(&input.artifact_version)?;
        if input.batch_size == 0 || input.concurrency == 0 || input.concurrency > input.batch_size {
            return Err(AppError::InvalidConfig(
                "批次和并发必须大于0，且并发不能超过批次".into(),
            ));
        }
        if input.mode == DeploymentMode::ServiceUpgrade {
            let service = input
                .service_name
                .as_deref()
                .map(str::trim)
                .filter(|service| SERVICE_WHITELIST.contains(service))
                .ok_or_else(|| AppError::InvalidConfig("单服升级服务名不受支持".into()))?;
            if input.image_name.as_deref().is_none_or(str::is_empty) {
                return Err(AppError::InvalidConfig("单服升级镜像名不能为空".into()));
            }
            input.service_name = Some(service.to_string());
        }
        let steps = match input.mode {
            DeploymentMode::FirstDeploy => vec![
                step("precheck", "远端环境预检", Some("precheck"), 10),
                step("upload", "上传Release与配置", None, 30),
                step("install", "安装并启动完整Release", Some("install"), 35),
                step("register", "调用一体机本地注册接口", None, 10),
                step("health", "服务与版本检查", Some("health"), 15),
            ],
            DeploymentMode::FullUpgrade => vec![
                step("precheck", "远端环境预检", Some("precheck"), 10),
                step("backup", "备份当前配置与必要数据", Some("backup"), 15),
                step("upload", "上传Release与配置", None, 25),
                step("install", "升级并重建服务", Some("install"), 35),
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
            artifact_path: input.artifact_path,
            artifact_name: input.artifact_name,
            artifact_version: input.artifact_version,
            service_name: input.service_name,
            image_name: input.image_name,
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
                Some(AgentInvocation {
                    action,
                    environment,
                })
            })
            .collect()
    }
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

    use super::{DeploymentMode, DeploymentPlan, DeploymentPlanInput};

    fn input(mode: DeploymentMode) -> DeploymentPlanInput {
        DeploymentPlanInput {
            mode,
            target_macs: vec!["001122334455".into()],
            artifact_path: "C:/release".into(),
            artifact_name: "Release".into(),
            artifact_version: "2026.08.28".into(),
            service_name: None,
            image_name: None,
            images: BTreeMap::new(),
            batch_size: 4,
            concurrency: 2,
        }
    }

    #[test]
    fn three_modes_have_fixed_agent_whitelists_and_weights() {
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
    fn service_upgrade_rejects_arbitrary_service_and_invalid_concurrency() {
        let mut invalid = input(DeploymentMode::ServiceUpgrade);
        invalid.service_name = Some("$(bad)".into());
        invalid.image_name = Some("bad:1".into());
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
