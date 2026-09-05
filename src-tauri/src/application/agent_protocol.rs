use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::application::ports::remote_command::ExecRequest;
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::AgentInvocation;
use crate::domain::aio::service_check::ServiceCheckReport;

const AGENT_ACTIONS: &[&str] = &[
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub step: String,
    pub status: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub code: Option<i32>,
}

pub fn agent_exec_request(
    remote_agent: &str,
    invocation: AgentInvocation,
) -> AppResult<ExecRequest> {
    if !remote_agent.starts_with('/') || remote_agent.contains("..") {
        return Err(AppError::InvalidConfig("远端Agent路径不安全".into()));
    }
    if !AGENT_ACTIONS.contains(&invocation.action.as_str()) {
        return Err(AppError::InvalidConfig("Agent action不在白名单".into()));
    }
    let request = ExecRequest {
        program: "sh".into(),
        args: vec![remote_agent.into(), invocation.action],
        env: invocation.environment,
        stdin: None,
        total_timeout: Duration::from_secs(30 * 60),
        inactivity_timeout: Duration::from_secs(3 * 60),
    };
    request.validate()?;
    Ok(request)
}

pub fn parse_agent_events(output: &str) -> AppResult<Vec<AgentEvent>> {
    let mut events = Vec::new();
    let mut structured = 0_u32;
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let value = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(value) => value,
            Err(_) => {
                events.push(AgentEvent {
                    step: "agent_output".into(),
                    status: "info".into(),
                    message: line.into(),
                    code: None,
                });
                continue;
            }
        };
        if value.get("protocolVersion").is_some() && value.get("step").is_none() {
            continue;
        }
        let event = serde_json::from_value::<AgentEvent>(value)
            .map_err(|_| AppError::InvalidConfig("Agent事件字段不完整".into()))?;
        if event.step.trim().is_empty() || event.status.trim().is_empty() {
            return Err(AppError::InvalidConfig("Agent事件步骤或状态为空".into()));
        }
        structured += 1;
        events.push(event);
    }
    if structured == 0 {
        return Err(AppError::InvalidConfig("Agent输出没有结构化事件".into()));
    }
    Ok(events)
}

/// 通过现有SSH标准输入执行只读采集，不上传Agent、不创建部署目录。
pub fn inspect_services_request(
    agent_source: &str,
    deploy_root: &str,
    source: &str,
    service_name: Option<&str>,
) -> AppResult<ExecRequest> {
    if !deploy_root.starts_with('/')
        || deploy_root == "/"
        || deploy_root
            .split('/')
            .any(|part| matches!(part, "." | ".."))
        || !matches!(
            source,
            "manual" | "first_deploy" | "full_upgrade" | "service_upgrade" | "rollback"
        )
        || service_name.is_some_and(|name| {
            name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
    {
        return Err(AppError::InvalidConfig("只读服务检查参数无效".into()));
    }
    let mut env = std::collections::BTreeMap::from([
        ("DEPLOY_ROOT".into(), deploy_root.into()),
        ("SERVICE_CHECK_SOURCE".into(), source.into()),
    ]);
    if let Some(service) = service_name {
        env.insert("SERVICE_NAME".into(), service.into());
    }
    let request = ExecRequest {
        program: "sh".into(),
        args: vec!["-s".into(), "--".into(), "inspect-services".into()],
        env,
        stdin: Some(agent_source.as_bytes().to_vec()),
        total_timeout: Duration::from_secs(120),
        inactivity_timeout: Duration::from_secs(90),
    };
    request.validate()?;
    Ok(request)
}

pub fn parse_service_check_report(output: &str) -> AppResult<ServiceCheckReport> {
    parse_service_check_report_with_time(output, None)
}

/// 先使用工作台采集时间规范化，再验证报告，远端时钟不参与有效性和先后裁决。
pub fn parse_service_check_report_at(
    output: &str,
    started_at: &str,
    checked_at: &str,
) -> AppResult<ServiceCheckReport> {
    parse_service_check_report_with_time(output, Some((started_at, checked_at)))
}

fn parse_service_check_report_with_time(
    output: &str,
    observed_time: Option<(&str, &str)>,
) -> AppResult<ServiceCheckReport> {
    let mut report = None;
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("step").and_then(serde_json::Value::as_str) != Some("service_observation") {
            continue;
        }
        let mut parsed = serde_json::from_value::<ServiceCheckReport>(
            value
                .get("report")
                .cloned()
                .ok_or_else(|| AppError::InvalidConfig("Agent服务检查报告缺失".into()))?,
        )
        .map_err(|_| AppError::InvalidConfig("Agent服务检查报告格式无效".into()))?;
        if let Some((started_at, checked_at)) = observed_time {
            parsed.started_at = started_at.into();
            parsed.checked_at = checked_at.into();
            for service in &mut parsed.services {
                service.checked_at = checked_at.into();
            }
        }
        parsed.validate()?;
        report = Some(parsed);
    }
    report.ok_or_else(|| AppError::InvalidConfig("Agent未返回真实服务检查报告".into()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::domain::aio::deployment::AgentInvocation;

    use super::{
        agent_exec_request, inspect_services_request, parse_agent_events,
        parse_service_check_report,
    };

    fn observation_output(source: &str, service: Option<&str>) -> String {
        serde_json::json!({
            "step": "service_observation", "status": "info", "report": {
                "startedAt": "2026-09-05T01:00:00Z", "checkedAt": "2026-09-05T01:00:01Z",
                "source": source, "scope": if service.is_some() {"service"} else {"all"},
                "serviceName": service, "expectedServices": ["edge"],
                "services": [{"serviceName": "edge", "state": "version_mismatch",
                    "runtimeState": "running", "healthStatus": "healthy",
                    "expectedImage": "repo/edge:2", "actualImage": "repo/edge:1", "imageId": "sha256:real-container-id",
                    "message": "实际镜像与生效Compose不一致", "checkedAt": "2026-09-05T01:00:01Z", "source": source}],
                "state": "succeeded", "error": null
            }
        }).to_string()
    }

    #[test]
    fn inspection_uses_stdin_and_returns_real_image_for_all_deployment_modes() {
        for (source, service) in [
            ("first_deploy", None),
            ("full_upgrade", None),
            ("service_upgrade", Some("edge")),
        ] {
            let request =
                inspect_services_request("#!/bin/sh\n", "/opt/data/deploy", source, service)
                    .unwrap();
            assert_eq!(request.args, ["-s", "--", "inspect-services"]);
            assert_eq!(request.stdin, Some(b"#!/bin/sh\n".to_vec()));
            assert!(!request.env.contains_key("REMOTE_IMAGE"));
            let report = parse_service_check_report(&observation_output(source, service)).unwrap();
            assert_eq!(
                report.services[0].actual_image.as_deref(),
                Some("repo/edge:1")
            );
            assert_eq!(report.services[0].state, "version_mismatch");
            assert_eq!(report.source, source);
            assert_eq!(report.service_name.as_deref(), service);
        }
    }

    #[test]
    fn report_parser_rejects_unobserved_output_and_invalid_scope() {
        assert!(
            parse_service_check_report("{\"step\":\"health\",\"status\":\"success\"}").is_err()
        );
        let output = observation_output("manual", Some("other"));
        assert!(parse_service_check_report(&output).is_err());
        assert!(inspect_services_request("agent", "/opt/../data", "manual", None).is_err());
        assert!(inspect_services_request("agent", "/opt/data", "manual", Some("edge;up")).is_err());
    }

    #[test]
    fn workbench_time_is_applied_before_validating_remote_clock_values() {
        let output = observation_output("rollback", Some("edge"))
            .replace("2026-09-05T01:00:00Z", "remote-clock-invalid")
            .replace("2026-09-05T01:00:01Z", "2020-01-01T08:00:00+08:00");
        assert!(parse_service_check_report(&output).is_err());
        let report = super::parse_service_check_report_at(
            &output,
            "2026-09-05T02:00:00.123456Z",
            "2026-09-05T02:00:01.123456Z",
        )
        .unwrap();
        assert_eq!(report.source, "rollback");
        assert_eq!(report.checked_at, "2026-09-05T02:00:01.123456Z");
        assert_eq!(report.services[0].checked_at, report.checked_at);
    }

    #[test]
    fn builds_only_whitelisted_agent_command_and_parses_json_lines() {
        let request = agent_exec_request(
            "/opt/data/edge-node-agent.sh",
            AgentInvocation {
                action: "service-upgrade".into(),
                environment: BTreeMap::from([
                    ("SERVICE_NAME".into(), "device-edge".into()),
                    ("SERVICE_IMAGE".into(), "device-edge:1".into()),
                ]),
            },
        )
        .expect("request");
        assert_eq!(request.program, "sh");
        assert_eq!(request.args[1], "service-upgrade");
        let events = parse_agent_events(
            "{\"step\":\"service_upgrade\",\"status\":\"running\",\"message\":\"start\"}\n\
             {\"step\":\"service_upgrade\",\"status\":\"success\",\"message\":\"done\"}\n",
        )
        .expect("events");
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn rejects_arbitrary_action_and_non_json_output() {
        assert!(
            agent_exec_request(
                "/tmp/agent.sh",
                AgentInvocation {
                    action: "rm".into(),
                    environment: BTreeMap::new(),
                },
            )
            .is_err()
        );
        assert!(parse_agent_events("plain output").is_err());
        let mixed = parse_agent_events(
            "NAME STATUS\n{\"step\":\"health\",\"status\":\"success\",\"message\":\"ok\"}\n",
        )
        .expect("mixed v1 output");
        assert_eq!(mixed.len(), 2);
    }
}
