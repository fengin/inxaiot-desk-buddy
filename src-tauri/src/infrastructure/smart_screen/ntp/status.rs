use super::*;

pub(super) async fn read<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
) -> AppResult<Value> {
    let text = device
        .shell(
            ip,
            &["dumpsys", "network_time_update_service"],
            CancellationToken::new(),
        )
        .await
        .map_err(|error| stage_error("读取系统授时服务状态", error))?;
    if text.len() > 128 * 1024
        || text.contains("Can't find service")
        || !(text.contains("NTP cache age:") || text.contains("LastNtpFetchTime:"))
    {
        return Err(AppError::Conflict(
            "当前固件未返回可核实的系统 NTP 授时状态".into(),
        ));
    }
    let uptime = device
        .shell(ip, &["cat", "/proc/uptime"], CancellationToken::new())
        .await
        .map_err(|error| stage_error("读取系统启动时长", error))?;
    let uptime_millis = uptime
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| (s * 1000.0) as u64);
    let mut status = parse(&text);
    status["uptimeMillis"] = json!(uptime_millis);
    status["systemServerPid"] = json!(
        device
            .shell(ip, &["pidof", "system_server"], CancellationToken::new())
            .await
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .filter(|pid| *pid > 0)
    );
    if let Some(fetch) = status["lastNtpFetchElapsedMillis"].as_u64() {
        status["cacheAgeMillis"] =
            json!(uptime_millis.and_then(|uptime| uptime.checked_sub(fetch)));
    }
    Ok(status)
}

fn parse(text: &str) -> Value {
    fn number(text: &str, key: &str) -> Option<u64> {
        text.lines().find_map(|line| {
            line.trim()
                .strip_prefix(key)
                .and_then(|s| s.trim().parse::<u64>().ok())
        })
    }
    let last_fetch = text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("LastNtpFetchTime:")
            .and_then(|s| parse_elapsed_duration(s.trim()))
    });
    let polling = text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("PollingIntervalMs:")
            .and_then(|s| parse_elapsed_duration(s.trim()))
    });
    // 4 寸只有成功取时后的启动内时间；10 寸提供缓存年龄与误差。两者都不打印运行中的服务器。
    json!({"cacheAgeMillis":number(text,"NTP cache age:"),"cacheCertaintyMillis":number(text,"NTP cache certainty:"),"lastNtpFetchElapsedMillis":last_fetch,"pollingIntervalMillis":polling})
}

pub(super) async fn source_event<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    server: &str,
    status: &Value,
) -> Option<Value> {
    if server.is_empty() || !fresh_sample(status) {
        return None;
    }
    let log = device
        .shell(
            ip,
            &[
                "logcat",
                "-d",
                "-b",
                "events",
                "-v",
                "brief",
                "-t",
                "200",
                "-s",
                "ntp_success",
            ],
            CancellationToken::new(),
        )
        .await
        .ok()?;
    if log.len() > 128 * 1024 {
        return None;
    }
    parse_source_event(&log, server, status)
}

fn parse_source_event(text: &str, server: &str, status: &Value) -> Option<Value> {
    let system_pid = status["systemServerPid"].as_u64()?;
    let format = regex::Regex::new(
        r"^\s*I/ntp_success\s*\(\s*(\d+)\s*\):\s*\[([^,]+),\s*(-?\d+),\s*(-?\d+)\]\s*$",
    )
    .ok()?;
    let latest=text.lines().filter_map(|line| {
        let captures=format.captures(line)?;
        let pid=captures[1].parse::<u64>().ok()?;
        if pid != system_pid { return None; }
        let rtt=captures[3].parse::<i64>().ok()?;
        let offset=captures[4].parse::<i64>().ok()?;
        (rtt >= 0).then(||json!({"address":captures[2].trim(),"systemServerPid":pid,"roundTripMillis":rtt,"offsetMillis":offset}))
    }).last()?;
    let address = latest["address"].as_str()?;
    let (host, ip) = address.rsplit_once('/')?;
    let matched = if let Ok(target) = server.parse::<std::net::IpAddr>() {
        ip.parse::<std::net::IpAddr>().ok() == Some(target)
    } else {
        host.trim_end_matches('.')
            .eq_ignore_ascii_case(server.trim_end_matches('.'))
    };
    // 普通内存 events 不读取跨启动的 pstore；当前系统服务 PID 与有效缓存独立核对。
    // 不能用错误时间校正后的 logcat monotonic 文本判断启动内时间。
    matched.then_some(latest)
}

fn parse_elapsed_duration(text: &str) -> Option<u64> {
    let format =
        regex::Regex::new(r"^\+(?:(\d+)d)?(?:(\d+)h)?(?:(\d+)m)?(?:(\d+)s)?(?:(\d+)ms)?$").ok()?;
    let captures = format.captures(text)?;
    let mut total = 0u64;
    for (index, factor) in [
        (1, 86_400_000u64),
        (2, 3_600_000),
        (3, 60_000),
        (4, 1000),
        (5, 1),
    ] {
        if let Some(value) = captures.get(index) {
            total = total.checked_add(value.as_str().parse::<u64>().ok()?.checked_mul(factor)?)?;
        }
    }
    (total > 0).then_some(total)
}
pub(super) fn fresh_sample(status: &Value) -> bool {
    status["cacheAgeMillis"].as_u64().is_some_and(|age| {
        status["pollingIntervalMillis"]
            .as_u64()
            .is_some_and(|polling| polling <= 86_400_000 && age < polling)
            && status["uptimeMillis"]
                .as_u64()
                .is_some_and(|uptime| age <= uptime)
    }) && (status["cacheCertaintyMillis"]
        .as_u64()
        .is_some_and(|certainty| certainty <= 15_000)
        || status["lastNtpFetchElapsedMillis"]
            .as_u64()
            .is_some_and(|fetch| fetch > 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_cache_and_old_samples_never_confirm_sync() {
        for age in [u64::MAX, i64::MAX as u64, 86_400_000] {
            let mut status = parse(&format!(
                "PollingIntervalMs: +1d0h0m0s0ms\nNTP cache age: {age}\nNTP cache certainty: 8\n"
            ));
            status["uptimeMillis"] = json!(300_000);
            assert!(!fresh_sample(&status));
        }
        assert!(!fresh_sample(&parse("")));
    }
    #[test]
    fn new_boot_cache_requires_real_sample_and_bounded_certainty() {
        let mut status =
            parse("PollingIntervalMs: +1d0h0m0s0ms\nNTP cache age: 1024\nNTP cache certainty: 8\n");
        status["uptimeMillis"] = json!(5000);
        assert!(fresh_sample(&status));
        status["uptimeMillis"] = json!(1000);
        assert!(!fresh_sample(&status));
        status["uptimeMillis"] = json!(5000);
        status["cacheCertaintyMillis"] = json!(15_001);
        assert!(!fresh_sample(&status));
    }
    #[test]
    fn four_inch_success_timestamp_is_elapsed_time_not_wall_clock_or_cache_age() {
        assert_eq!(parse_elapsed_duration("+1m48s777ms"), Some(108_777));
        assert_eq!(parse_elapsed_duration("+12h3m5s729ms"), Some(43_385_729));
        for missing in ["-1ms", "0", "+0ms", "invalid", "+"] {
            assert!(parse_elapsed_duration(missing).is_none());
        }
        let mut status = parse("PollingIntervalMs: +12h0m0s0ms\nLastNtpFetchTime: +1m48s777ms\n");
        status["uptimeMillis"] = json!(110_000);
        status["cacheAgeMillis"] = json!(1223);
        assert!(fresh_sample(&status));
    }
    #[test]
    fn direct_source_event_requires_matching_boot_sample_and_screen_resolved_address() {
        let status = json!({"uptimeMillis":110_000,"cacheAgeMillis":1223,"systemServerPid":501});
        let direct = "I/ntp_success( 501): [/192.168.3.142,8,-300000]";
        assert!(parse_source_event(direct, "192.168.3.142", &status).is_some());
        assert!(parse_source_event(direct, "other.internal", &status).is_none());
        let domain = "I/ntp_success(501): [ntp.internal/192.168.3.142,8,123]";
        assert!(parse_source_event(domain, "ntp.internal", &status).is_some());
        for unrelated in [
            "I/ntp_success(502): [/192.168.3.142,8,123]",
            "I/ntp_success(501): [/192.168.3.142,-8,123]",
            "I/ntp_success(501): [/192.168.3.143,8,123]",
        ] {
            assert!(parse_source_event(unrelated, "192.168.3.142", &status).is_none());
        }
        assert!(
            parse_source_event(
                &format!("{direct}\nI/ntp_success(501): [/192.168.3.143,8,123]"),
                "192.168.3.142",
                &status
            )
            .is_none()
        );
    }
}
