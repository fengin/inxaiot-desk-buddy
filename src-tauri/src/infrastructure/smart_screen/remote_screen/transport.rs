use super::super::device::{AdbDevice, AndroidTools};
use super::png::MAX_PNG_BYTES;
use crate::core::error::{AppError, AppResult};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

pub trait ScreenCapturePort: Send + Sync {
    fn capture(
        &self,
        ip: &str,
        cancel: CancellationToken,
    ) -> impl std::future::Future<Output = AppResult<Vec<u8>>> + Send;

    fn capture_verified(
        &self,
        ip: &str,
        cancel: CancellationToken,
    ) -> impl std::future::Future<Output = AppResult<RawCapture>> + Send {
        async move {
            Ok(RawCapture {
                png: self.capture(ip, cancel).await?,
                serial: None,
            })
        }
    }
}

pub struct RawCapture {
    pub png: Vec<u8>,
    pub serial: Option<String>,
}

pub struct NativeScreenCapture {
    pub tools: AndroidTools,
}

impl ScreenCapturePort for NativeScreenCapture {
    async fn capture(&self, ip: &str, cancel: CancellationToken) -> AppResult<Vec<u8>> {
        let device = AdbDevice::new(self.tools.clone());
        device.connect(ip, cancel.clone()).await?;
        let address = ip
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| AppError::InvalidConfig("设备 IP 必须是有效 IPv4 地址".into()))?;
        run_bytes(
            &self.tools.adb,
            &[
                "-s".into(),
                format!("{address}:5555"),
                "exec-out".into(),
                "screencap".into(),
                "-p".into(),
            ],
            Duration::from_secs(12),
            cancel,
            MAX_PNG_BYTES,
            64 * 1024,
        )
        .await
    }

    async fn capture_verified(&self, ip: &str, cancel: CancellationToken) -> AppResult<RawCapture> {
        let device = AdbDevice::new(self.tools.clone());
        device.connect(ip, cancel.clone()).await?;
        let before = super::input::trusted_serial(
            &device
                .shell(ip, &["getprop", "ro.serialno"], cancel.clone())
                .await?,
        );
        let address = ip
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| AppError::InvalidConfig("设备地址无效".into()))?;
        let png = run_bytes(
            &self.tools.adb,
            &[
                "-s".into(),
                format!("{address}:5555"),
                "exec-out".into(),
                "screencap".into(),
                "-p".into(),
            ],
            Duration::from_secs(12),
            cancel.clone(),
            MAX_PNG_BYTES,
            64 * 1024,
        )
        .await?;
        let after = super::input::trusted_serial(
            &device
                .shell(ip, &["getprop", "ro.serialno"], cancel)
                .await?,
        );
        if before != after {
            return Err(AppError::Conflict(
                "设备身份在采集期间改变，请重新打开屏幕".into(),
            ));
        }
        Ok(RawCapture { png, serial: after })
    }
}

/// 只有类型化动作编码后的脚本可以进入此方法；文字不放入本机ADB参数，不回显任何输出。
pub(super) async fn run_stdin(
    program: &Path,
    arguments: &[String],
    input: &[u8],
    limit: Duration,
    cancel: CancellationToken,
) -> AppResult<()> {
    run_stdin_output(program, arguments, input, limit, cancel)
        .await
        .map(|_| ())
}

/// 受控辅助程序的有界状态通道；调用方只解析固定标记，禁止回显原输出。
pub(super) async fn run_stdin_output(
    program: &Path,
    arguments: &[String],
    input: &[u8],
    limit: Duration,
    cancel: CancellationToken,
) -> AppResult<Vec<u8>> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let mut command = tokio::process::Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .map_err(|error| AppError::io("启动远程输入工具", &error))?;
    let mut stdin = child.stdin.take().ok_or(AppError::Io {
        operation: "准备远程输入通道",
    })?;
    let stdout = child.stdout.take().ok_or(AppError::Io {
        operation: "准备远程输入结果",
    })?;
    let stderr = child.stderr.take().ok_or(AppError::Io {
        operation: "准备远程输入状态",
    })?;
    let outcome = {
        let operation = async {
            let (_, stdout, _, status) = tokio::try_join!(
                async move {
                    stdin
                        .write_all(input)
                        .await
                        .map_err(|e| AppError::io("发送远程输入", &e))?;
                    stdin
                        .shutdown()
                        .await
                        .map_err(|e| AppError::io("结束远程输入", &e))?;
                    drop(stdin);
                    Ok::<(), AppError>(())
                },
                read_limited(stdout, 64 * 1024),
                read_limited(stderr, 64 * 1024),
                async {
                    child
                        .wait()
                        .await
                        .map_err(|e| AppError::io("等待远程输入结果", &e))
                },
            )?;
            if status.success() {
                Ok(stdout)
            } else {
                Err(AppError::Conflict(
                    "远程输入结果未确认，请刷新屏幕核对".into(),
                ))
            }
        };
        tokio::select! {
            biased;
            _=cancel.cancelled()=>Err(AppError::Cancelled),
            result=tokio::time::timeout(limit,operation)=>result.unwrap_or_else(|_|Err(AppError::timeout("等待远程输入结果"))),
        }
    };
    if outcome.is_err() {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    }
    outcome
}

async fn read_limited<R: AsyncRead + Unpin>(mut stream: R, maximum: usize) -> AppResult<Vec<u8>> {
    let mut result = Vec::with_capacity(maximum.min(64 * 1024));
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let length = stream
            .read(&mut buffer)
            .await
            .map_err(|error| AppError::io("读取屏幕采集结果", &error))?;
        if length == 0 {
            return Ok(result);
        }
        if result.len().saturating_add(length) > maximum {
            return Err(AppError::Conflict(
                "屏幕采集数据超过限制，请停止自动刷新后重试".into(),
            ));
        }
        result.extend_from_slice(&buffer[..length]);
    }
}

/// 截图专用有界字节通道；不经过旧设备命令的 UTF-8 文本转换，也不写临时文件。
pub(super) async fn run_bytes(
    program: &Path,
    arguments: &[String],
    limit: Duration,
    cancel: CancellationToken,
    maximum_stdout: usize,
    maximum_stderr: usize,
) -> AppResult<Vec<u8>> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let mut command = tokio::process::Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .map_err(|error| AppError::io("启动屏幕采集工具", &error))?;
    let stdout = child.stdout.take().ok_or(AppError::Io {
        operation: "读取屏幕采集标准输出",
    })?;
    let stderr = child.stderr.take().ok_or(AppError::Io {
        operation: "读取屏幕采集错误输出",
    })?;
    let result = {
        let operation = async {
            let (stdout, _stderr, status) = tokio::try_join!(
                read_limited(stdout, maximum_stdout),
                read_limited(stderr, maximum_stderr),
                async {
                    child
                        .wait()
                        .await
                        .map_err(|error| AppError::io("等待屏幕采集工具结束", &error))
                },
            )?;
            if !status.success() {
                // 不回显进程输出；截图内容及设备任意输出都不进入日志或错误摘要。
                return Err(AppError::Conflict(
                    "无法获取屏幕，请确认设备在线后重试".into(),
                ));
            }
            Ok(stdout)
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(AppError::Cancelled),
            result = tokio::time::timeout(limit, operation) => result.unwrap_or_else(|_| Err(AppError::timeout("获取当前屏幕"))),
        }
    };
    if result.is_err() {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    }
    result
}
