use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::application::ports::remote_command::ExecRequest;
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::AgentInvocation;

const AGENT_ACTIONS: &[&str] = &[
    "version",
    "precheck",
    "install",
    "backup",
    "health",
    "service-check",
    "service-upgrade",
    "service-health",
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::domain::aio::deployment::AgentInvocation;

    use super::{agent_exec_request, parse_agent_events};

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
