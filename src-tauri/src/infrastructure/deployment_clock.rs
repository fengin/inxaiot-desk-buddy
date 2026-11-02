use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;

use crate::application::ports::remote_command::{
    ExecRequest, NoopRemoteOutputSink, RemoteCommandExecutor,
};
use crate::core::error::{AppError, AppResult};

const CLOCK_TOLERANCE_MS: i128 = 5_000;
const MAX_SAMPLE_DURATION: Duration = Duration::from_secs(2);
const MAX_LOCAL_CLOCK_JUMP_MS: i128 = 1_000;
// 只设置系统时刻；不改变时区、NTP 配置或硬件时钟，也不向 sudo 传递密码。
const SET_CLOCK_SCRIPT: &str = r#"if [ "$(id -u)" = 0 ]; then
    exec date -u -s "$1" '+%s%3N'
else
    exec sudo -n date -u -s "$1" '+%s%3N'
fi"#;

pub(super) enum ClockSyncEvent {
    Checking,
    DriftDetected { offset_ms: i128 },
    Verified { offset_ms: i128, corrected: bool },
}

struct ClockMoment {
    unix_ms: i128,
    monotonic: Instant,
}

impl ClockMoment {
    fn now() -> AppResult<Self> {
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i128::try_from(duration.as_millis()).ok())
            .ok_or_else(|| {
                AppError::InvalidConfig("当前电脑系统时间无效，无法校准一体机".into())
            })?;
        Ok(Self {
            unix_ms,
            monotonic: Instant::now(),
        })
    }
}

pub(super) async fn synchronize_host_clock<R, F, Fut, O>(
    remote: &R,
    cancellation: &CancellationToken,
    checkpoint: &F,
    observe: O,
) -> AppResult<()>
where
    R: RemoteCommandExecutor + ?Sized,
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = AppResult<()>> + Send,
    O: Fn(ClockSyncEvent) -> AppResult<()>,
{
    synchronize_with_clock(remote, cancellation, checkpoint, observe, ClockMoment::now).await
}

async fn synchronize_with_clock<R, F, Fut, O, N>(
    remote: &R,
    cancellation: &CancellationToken,
    checkpoint: &F,
    observe: O,
    now: N,
) -> AppResult<()>
where
    R: RemoteCommandExecutor + ?Sized,
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = AppResult<()>> + Send,
    O: Fn(ClockSyncEvent) -> AppResult<()>,
    N: Fn() -> AppResult<ClockMoment>,
{
    observe(ClockSyncEvent::Checking)?;
    check_cancelled(cancellation)?;
    checkpoint().await?;
    let offset_ms = read_clock_offset(remote, cancellation, &now).await?;
    if offset_ms.abs() <= CLOCK_TOLERANCE_MS {
        return observe(ClockSyncEvent::Verified {
            offset_ms,
            corrected: false,
        });
    }

    observe(ClockSyncEvent::DriftDetected { offset_ms })?;
    check_cancelled(cancellation)?;
    checkpoint().await?;
    check_cancelled(cancellation)?;
    // 上传和前一次探测可能耗时，写命令发出前重新读取电脑时间。
    let unix_ms = now()?.unix_ms;
    let timestamp = format!("@{}.{:03}", unix_ms / 1_000, unix_ms % 1_000);
    let request = command(
        "sh",
        vec![
            "-c".into(),
            SET_CLOCK_SCRIPT.into(),
            "inx-clock-sync".into(),
            timestamp,
        ],
    );
    let result = remote
        .run(&request, cancellation, &NoopRemoteOutputSink)
        .await?;
    check_cancelled(cancellation)?;
    if result.exit_status != 0 {
        return Err(AppError::Conflict(format!(
            "自动校准一体机时钟失败（退出码={}），请检查 SSH 账号的校时权限（root 或免交互 sudo）及 date 命令",
            result.exit_status,
        )));
    }

    checkpoint().await?;
    let offset_ms = read_clock_offset(remote, cancellation, &now).await?;
    if offset_ms.abs() > CLOCK_TOLERANCE_MS {
        return Err(AppError::Conflict(format!(
            "一体机时钟校准后回读与当前电脑的估算偏差仍为{offset_ms:+}毫秒，超过允许的5秒，已停止该台部署",
        )));
    }
    observe(ClockSyncEvent::Verified {
        offset_ms,
        corrected: true,
    })
}

async fn read_clock_offset<R, N>(
    remote: &R,
    cancellation: &CancellationToken,
    now: &N,
) -> AppResult<i128>
where
    R: RemoteCommandExecutor + ?Sized,
    N: Fn() -> AppResult<ClockMoment>,
{
    check_cancelled(cancellation)?;
    let before = now()?;
    let result = remote
        .run(
            &command("date", vec!["-u".into(), "+%s%3N".into()]),
            cancellation,
            &NoopRemoteOutputSink,
        )
        .await?;
    let after = now()?;
    check_cancelled(cancellation)?;
    if result.exit_status != 0 {
        return Err(AppError::Conflict(format!(
            "读取一体机系统时间失败（退出码={}），已停止该台部署",
            result.exit_status,
        )));
    }
    let elapsed = after
        .monotonic
        .checked_duration_since(before.monotonic)
        .ok_or_else(|| AppError::Conflict("无法核实当前电脑的计时结果，已停止该台部署".into()))?;
    if elapsed > MAX_SAMPLE_DURATION {
        return Err(AppError::Conflict(
            "读取一体机时钟耗时超过2秒，无法可靠判断时间偏差，已停止该台部署，请检查网络后重试"
                .into(),
        ));
    }
    let local_elapsed_ms = after.unix_ms - before.unix_ms;
    if (local_elapsed_ms - elapsed.as_millis() as i128).abs() > MAX_LOCAL_CLOCK_JUMP_MS {
        return Err(AppError::Conflict(
            "核对期间当前电脑系统时间发生跳变，已停止该台部署，请确认电脑时间稳定后重试".into(),
        ));
    }
    let remote_ms = result
        .stdout
        .trim()
        .parse::<u64>()
        .map(i128::from)
        .map_err(|_| AppError::Conflict("无法解析一体机毫秒时间戳，已停止该台部署".into()))?;
    // 用请求往返中点减小网络耗时影响；UTC 时间戳不依赖双方显示时区。
    Ok(remote_ms - (before.unix_ms + local_elapsed_ms / 2))
}

fn command(program: &str, args: Vec<String>) -> ExecRequest {
    ExecRequest {
        program: program.into(),
        args,
        env: BTreeMap::new(),
        stdin: None,
        total_timeout: Duration::from_secs(10),
        inactivity_timeout: Duration::from_secs(10),
    }
}

fn check_cancelled(cancellation: &CancellationToken) -> AppResult<()> {
    if cancellation.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::remote_command::{RemoteCommandResult, RemoteOutputSink};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    const BASE: i128 = 1_793_615_000_000;

    #[derive(Default)]
    struct FakeRemote {
        responses: Mutex<VecDeque<(u32, String)>>,
        requests: Mutex<Vec<ExecRequest>>,
    }

    impl FakeRemote {
        fn new(responses: impl IntoIterator<Item = (u32, String)>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                ..Self::default()
            }
        }
        fn writes(&self) -> usize {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.program == "sh")
                .count()
        }
    }

    impl RemoteCommandExecutor for FakeRemote {
        async fn run(
            &self,
            request: &ExecRequest,
            _: &CancellationToken,
            _: &dyn RemoteOutputSink,
        ) -> AppResult<RemoteCommandResult> {
            request.validate()?;
            self.requests.lock().unwrap().push(request.clone());
            let (exit_status, stdout) = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("意外的远端命令");
            Ok(RemoteCommandResult {
                exit_status,
                stdout,
                stderr: String::new(),
                duration_ms: 0,
            })
        }
    }

    fn moments(values: &[(i128, u64)]) -> impl Fn() -> AppResult<ClockMoment> {
        let start = Instant::now();
        let queue = Mutex::new(
            values
                .iter()
                .map(|&(unix_ms, elapsed)| ClockMoment {
                    unix_ms,
                    monotonic: start + Duration::from_millis(elapsed),
                })
                .collect::<VecDeque<_>>(),
        );
        move || {
            Ok(queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("意外的电脑时钟读取"))
        }
    }

    #[tokio::test]
    async fn clocks_within_tolerance_do_not_write_and_use_round_trip_midpoint() {
        for drift in [-5_000, 0, 5_000] {
            let remote = FakeRemote::new([(0, (BASE + 100 + drift).to_string())]);
            synchronize_with_clock(
                &remote,
                &CancellationToken::new(),
                &|| async { Ok(()) },
                |_| Ok(()),
                moments(&[(BASE, 0), (BASE + 200, 200)]),
            )
            .await
            .unwrap();
            assert_eq!(remote.writes(), 0);
        }
    }

    #[tokio::test]
    async fn fast_and_slow_clocks_are_corrected_with_fresh_time_and_verified() {
        for drift in [-120_000, 120_000] {
            let remote = FakeRemote::new([
                (0, (BASE + drift).to_string()),
                (0, String::new()),
                (0, (BASE + 90_010).to_string()),
            ]);
            let events = Mutex::new(Vec::new());
            synchronize_with_clock(
                &remote,
                &CancellationToken::new(),
                &|| async { Ok(()) },
                |event| {
                    events.lock().unwrap().push(event);
                    Ok(())
                },
                moments(&[
                    (BASE, 0),
                    (BASE, 0),
                    (BASE + 90_000, 90_000),
                    (BASE + 90_000, 90_000),
                    (BASE + 90_020, 90_020),
                ]),
            )
            .await
            .unwrap();
            assert_eq!(remote.writes(), 1);
            let requests = remote.requests.lock().unwrap();
            assert_eq!(
                requests[1].args[3],
                format!("@{}.{:03}", (BASE + 90_000) / 1000, (BASE + 90_000) % 1000)
            );
            assert!(requests[1].stdin.is_none());
            assert!(SET_CLOCK_SCRIPT.contains("sudo -n date"));
            assert!(matches!(
                events.lock().unwrap().last(),
                Some(ClockSyncEvent::Verified {
                    corrected: true,
                    offset_ms: 0
                })
            ));
        }
    }

    #[tokio::test]
    async fn denied_write_and_failed_readback_never_report_success_or_retry_write() {
        for (write_exit, readback) in [(1, None), (0, Some(BASE + 120_000))] {
            let mut responses = vec![
                (0, (BASE - 120_000).to_string()),
                (write_exit, String::new()),
            ];
            if let Some(value) = readback {
                responses.push((0, value.to_string()));
            }
            let remote = FakeRemote::new(responses);
            let verified = Mutex::new(false);
            let result = synchronize_with_clock(
                &remote,
                &CancellationToken::new(),
                &|| async { Ok(()) },
                |event| {
                    if matches!(event, ClockSyncEvent::Verified { .. }) {
                        *verified.lock().unwrap() = true;
                    }
                    Ok(())
                },
                moments(&[(BASE, 0), (BASE, 0), (BASE, 0), (BASE, 0), (BASE, 0)]),
            )
            .await;
            assert!(result.is_err());
            assert_eq!(remote.writes(), 1);
            assert!(!*verified.lock().unwrap());
        }
    }

    #[tokio::test]
    async fn malformed_failed_slow_or_unstable_clock_reads_never_write() {
        let cases = [
            (0, "1793615000%3N".into(), BASE, 0),
            (1, String::new(), BASE, 0),
            (0, BASE.to_string(), BASE + 2_001, 2_001),
            (0, BASE.to_string(), BASE + 60_000, 100),
            (0, BASE.to_string(), BASE - 60_000, 100),
        ];
        for (exit, output, after, elapsed) in cases {
            let remote = FakeRemote::new([(exit, output)]);
            let result = synchronize_with_clock(
                &remote,
                &CancellationToken::new(),
                &|| async { Ok(()) },
                |_| Ok(()),
                moments(&[(BASE, 0), (after, elapsed)]),
            )
            .await;
            assert!(result.is_err());
            assert_eq!(remote.writes(), 0);
        }
    }

    #[tokio::test]
    async fn cancellation_and_lost_lease_before_write_prevent_clock_changes() {
        for cancel in [true, false] {
            let remote = FakeRemote::new([(0, (BASE - 120_000).to_string())]);
            let token = CancellationToken::new();
            let calls = Mutex::new(0);
            let result = synchronize_with_clock(
                &remote,
                &token,
                &|| {
                    let mut calls = calls.lock().unwrap();
                    *calls += 1;
                    let fail = !cancel && *calls == 2;
                    async move {
                        if fail {
                            Err(AppError::Conflict("租约已失效".into()))
                        } else {
                            Ok(())
                        }
                    }
                },
                |event| {
                    if cancel && matches!(event, ClockSyncEvent::DriftDetected { .. }) {
                        token.cancel();
                    }
                    Ok(())
                },
                moments(&[(BASE, 0), (BASE, 0)]),
            )
            .await;
            if cancel {
                assert!(matches!(result, Err(AppError::Cancelled)));
            } else {
                assert!(result.unwrap_err().to_string().contains("租约"));
            }
            assert_eq!(remote.writes(), 0);
        }
        let remote = FakeRemote::default();
        let token = CancellationToken::new();
        token.cancel();
        let result = synchronize_with_clock(
            &remote,
            &token,
            &|| async { Ok(()) },
            |_| Ok(()),
            moments(&[]),
        )
        .await;
        assert!(matches!(result, Err(AppError::Cancelled)));
        assert!(remote.requests.lock().unwrap().is_empty());
    }
}
