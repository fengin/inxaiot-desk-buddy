use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::core::error::{AppError, AppResult};

/// 来自远端容器的一次观测，不能由部署期望版本推导。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceObservation {
    pub service_name: String,
    pub state: String,
    pub runtime_state: String,
    pub health_status: Option<String>,
    pub expected_image: Option<String>,
    pub actual_image: Option<String>,
    pub image_id: Option<String>,
    pub message: Option<String>,
    pub checked_at: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceCheckReport {
    pub started_at: String,
    pub checked_at: String,
    pub source: String,
    pub scope: String,
    pub service_name: Option<String>,
    pub expected_services: Vec<String>,
    pub services: Vec<ServiceObservation>,
    pub state: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeServiceCheckSnapshot {
    #[serde(default)]
    pub expected_services: Vec<String>,
    #[serde(default)]
    pub services: Vec<ServiceObservation>,
    pub last_full_check_at: Option<String>,
    pub last_attempt: Option<ServiceCheckReport>,
}

impl ServiceCheckReport {
    pub fn validate(&self) -> AppResult<()> {
        let started_at = parse_check_time(&self.started_at)?;
        let checked_at = parse_check_time(&self.checked_at)?;
        if checked_at < started_at {
            return Err(invalid_report("检查结束时间早于开始时间"));
        }
        if !matches!(
            self.source.as_str(),
            "first_deploy" | "full_upgrade" | "service_upgrade" | "manual" | "rollback"
        ) {
            return Err(invalid_report("检查来源无效"));
        }
        if !matches!(self.state.as_str(), "succeeded" | "failed") {
            return Err(invalid_report("检查结果无效"));
        }
        match self.scope.as_str() {
            "all" if self.service_name.is_none() => {}
            "service"
                if self
                    .service_name
                    .as_deref()
                    .is_some_and(|name| !name.trim().is_empty()) => {}
            _ => return Err(invalid_report("检查范围无效")),
        }
        let expected = self.expected_services.iter().collect::<BTreeSet<_>>();
        if expected.len() != self.expected_services.len()
            || expected.iter().any(|name| name.trim().is_empty())
            || (self.state == "succeeded" && expected.is_empty())
        {
            return Err(invalid_report("生效服务集合为空或含重复名称"));
        }
        let mut names = BTreeSet::new();
        for service in &self.services {
            if service.service_name.trim().is_empty() || !names.insert(&service.service_name) {
                return Err(invalid_report("服务名称为空或重复"));
            }
            if self.state == "succeeded" && !expected.contains(&service.service_name) {
                return Err(invalid_report("观测服务不在生效服务集合中"));
            }
            if self.scope == "service" && self.service_name.as_ref() != Some(&service.service_name)
            {
                return Err(invalid_report("单服务检查包含了其他服务"));
            }
            if !matches!(
                service.state.as_str(),
                "normal" | "abnormal" | "version_mismatch" | "unknown"
            ) {
                return Err(invalid_report("服务检查状态无效"));
            }
            parse_check_time(&service.checked_at)?;
            if service.source != self.source {
                return Err(invalid_report("服务观测来源与本次检查不一致"));
            }
        }
        if self.state == "succeeded" && self.scope == "service" && self.services.len() != 1 {
            return Err(invalid_report("单服务检查缺少目标服务结果"));
        }
        if self.state == "succeeded" && self.scope == "all" && names != expected {
            return Err(invalid_report("完整服务检查缺少生效服务结果"));
        }
        Ok(())
    }
}

impl NodeServiceCheckSnapshot {
    /// 返回 false 表示过时或重复结果，不得覆盖后发检查。
    pub fn apply_report(&mut self, report: &ServiceCheckReport) -> AppResult<bool> {
        report.validate()?;
        if let Some(previous) = &self.last_attempt {
            let incoming_start = parse_check_time(&report.started_at)?;
            let previous_start = parse_check_time(&previous.started_at)?;
            if incoming_start < previous_start
                || (incoming_start == previous_start
                    && parse_check_time(&report.checked_at)?
                        <= parse_check_time(&previous.checked_at)?)
            {
                return Ok(false);
            }
        }
        if report.state == "succeeded" {
            self.expected_services = report.expected_services.clone();
            self.expected_services.sort();
            if report.scope == "all" {
                self.services = report.services.clone();
                self.last_full_check_at = Some(report.checked_at.clone());
            } else {
                for incoming in &report.services {
                    if let Some(current) = self
                        .services
                        .iter_mut()
                        .find(|item| item.service_name == incoming.service_name)
                    {
                        *current = incoming.clone();
                    } else {
                        self.services.push(incoming.clone());
                    }
                }
            }
            self.services
                .sort_by(|left, right| left.service_name.cmp(&right.service_name));
        }
        // 失败只保存尝试，不把空结果或部分输出变成有效状态。
        self.last_attempt = Some(report.clone());
        Ok(true)
    }

    pub fn summary(&self) -> (String, String) {
        if self
            .last_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.state == "failed")
        {
            return ("unreachable".into(), "检查失败".into());
        }
        if self.expected_services.is_empty() {
            return ("unknown".into(), "暂无检查记录".into());
        }
        let active = self
            .services
            .iter()
            .filter(|item| self.expected_services.contains(&item.service_name))
            .collect::<Vec<_>>();
        let abnormal = active
            .iter()
            .filter(|item| item.state == "abnormal")
            .count();
        let mismatch = active
            .iter()
            .filter(|item| item.state == "version_mismatch")
            .count();
        if abnormal > 0 || mismatch > 0 {
            let mut labels = Vec::new();
            if abnormal > 0 {
                labels.push(format!("{abnormal}项异常"));
            }
            if mismatch > 0 {
                labels.push(format!("{mismatch}项版本不符"));
            }
            return ("warning".into(), labels.join("、"));
        }
        let checked = active.iter().filter(|item| item.state != "unknown").count();
        if checked < self.expected_services.len() {
            return (
                "unknown".into(),
                format!("已检查{checked}/{}项", self.expected_services.len()),
            );
        }
        (
            "healthy".into(),
            format!("{}项正常", self.expected_services.len()),
        )
    }
}

fn parse_check_time(value: &str) -> AppResult<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()
        .filter(|value| value.offset() == UtcOffset::UTC)
        .ok_or_else(|| invalid_report("检查时间必须为 RFC3339 UTC"))
}

fn invalid_report(message: &str) -> AppError {
    AppError::InvalidConfig(format!("服务检查记录无效：{message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(scope: &str, minute: u8, names: &[&str]) -> ServiceCheckReport {
        let checked_at = format!("2020-01-01T00:{minute:02}:01Z");
        ServiceCheckReport {
            started_at: format!("2020-01-01T00:{minute:02}:00Z"),
            checked_at: checked_at.clone(),
            source: "manual".into(),
            scope: scope.into(),
            service_name: (scope == "service").then(|| names[0].into()),
            expected_services: names.iter().map(|name| (*name).into()).collect(),
            services: names
                .iter()
                .map(|name| ServiceObservation {
                    service_name: (*name).into(),
                    state: "normal".into(),
                    runtime_state: "running".into(),
                    health_status: None,
                    expected_image: Some(format!("{name}:1")),
                    actual_image: Some(format!("{name}:1")),
                    image_id: Some("sha256:real".into()),
                    message: None,
                    checked_at: checked_at.clone(),
                    source: "manual".into(),
                })
                .collect(),
            state: "succeeded".into(),
            error: None,
        }
    }

    #[test]
    fn full_check_replaces_removed_services_and_does_not_expire() {
        let mut snapshot = NodeServiceCheckSnapshot::default();
        snapshot
            .apply_report(&report("all", 1, &["edge", "rule"]))
            .unwrap();
        snapshot.apply_report(&report("all", 2, &["edge"])).unwrap();
        assert_eq!(snapshot.services.len(), 1);
        assert_eq!(snapshot.summary(), ("healthy".into(), "1项正常".into()));
    }

    #[test]
    fn service_check_only_updates_target_and_keeps_original_check_times() {
        let mut snapshot = NodeServiceCheckSnapshot::default();
        let full = report("all", 1, &["edge", "rule"]);
        snapshot.apply_report(&full).unwrap();
        let mut single = report("service", 2, &["edge"]);
        single.expected_services.push("rule".into());
        single.services[0].state = "version_mismatch".into();
        snapshot.apply_report(&single).unwrap();
        assert_eq!(snapshot.last_full_check_at, Some(full.checked_at.clone()));
        assert_eq!(snapshot.services[1].checked_at, full.checked_at);
        assert_eq!(snapshot.summary(), ("warning".into(), "1项版本不符".into()));
    }

    #[test]
    fn failure_preserves_valid_observations_and_tracks_attempt() {
        let mut snapshot = NodeServiceCheckSnapshot::default();
        snapshot.apply_report(&report("all", 1, &["edge"])).unwrap();
        let original = snapshot.services.clone();
        let mut failed = report("all", 2, &[]);
        failed.state = "failed".into();
        failed.error = Some("读取容器失败".into());
        snapshot.apply_report(&failed).unwrap();
        assert_eq!(snapshot.services, original);
        assert_eq!(snapshot.last_attempt, Some(failed));
        assert_eq!(
            snapshot.summary(),
            ("unreachable".into(), "检查失败".into())
        );
    }

    #[test]
    fn late_completion_of_older_check_cannot_replace_newer_result() {
        let mut snapshot = NodeServiceCheckSnapshot::default();
        let new = report("all", 2, &["edge"]);
        snapshot.apply_report(&new).unwrap();
        let mut old = report("all", 1, &["rule"]);
        old.checked_at = "2020-01-01T00:03:00Z".into();
        assert!(!snapshot.apply_report(&old).unwrap());
        assert!(!snapshot.apply_report(&new).unwrap());
        assert_eq!(snapshot.last_attempt, Some(new));
    }

    #[test]
    fn partial_observations_do_not_claim_full_health() {
        let mut snapshot = NodeServiceCheckSnapshot::default();
        let mut single = report("service", 1, &["edge"]);
        single.expected_services.push("rule".into());
        snapshot.apply_report(&single).unwrap();
        assert_eq!(snapshot.summary(), ("unknown".into(), "已检查1/2项".into()));
        let mut full = report("all", 2, &["edge", "rule"]);
        full.services[1].state = "unknown".into();
        snapshot.apply_report(&full).unwrap();
        assert_eq!(snapshot.summary(), ("unknown".into(), "已检查1/2项".into()));
    }

    #[test]
    fn summary_uses_effective_services_without_discarding_unchecked_history() {
        let mut snapshot = NodeServiceCheckSnapshot::default();
        let mut full = report("all", 1, &["edge", "removed"]);
        full.services[1].state = "abnormal".into();
        snapshot.apply_report(&full).unwrap();
        snapshot
            .apply_report(&report("service", 2, &["edge"]))
            .unwrap();
        assert_eq!(snapshot.services.len(), 2);
        assert_eq!(snapshot.summary(), ("healthy".into(), "1项正常".into()));
        assert_eq!(snapshot.last_full_check_at, Some(full.checked_at));
    }

    #[test]
    fn invalid_report_does_not_mutate_snapshot() {
        let mut snapshot = NodeServiceCheckSnapshot::default();
        let mut invalid = report("service", 1, &["edge", "rule"]);
        assert!(snapshot.apply_report(&invalid).is_err());
        invalid.scope = "all".into();
        invalid.service_name = None;
        invalid.checked_at = "2020-01-01T08:01:01+08:00".into();
        assert!(snapshot.apply_report(&invalid).is_err());
        assert_eq!(snapshot, NodeServiceCheckSnapshot::default());
    }

    #[test]
    fn incomplete_full_report_cannot_claim_a_completed_full_check() {
        let mut snapshot = NodeServiceCheckSnapshot::default();
        let mut incomplete = report("all", 1, &["edge"]);
        incomplete.expected_services.push("rule".into());
        assert!(snapshot.apply_report(&incomplete).is_err());
        assert_eq!(snapshot, NodeServiceCheckSnapshot::default());
    }
}
