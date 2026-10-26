//! Android 10及以后使用系统无障碍节点动作；辅助JAR无凭据，可按内容哈希复用。
use super::{ControlAction, ControlKey};
use crate::core::error::{AppError, AppResult};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

const JAR: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/remote-input/focused-text.jar"
));
const EXPECTED_SHA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/remote-input/focused-text.sha256"
));
const TTL: Duration = Duration::from_secs(4);
const SETTLE: Duration = Duration::from_secs(5);

fn ascii_stdin(bytes: &[u8]) -> Zeroizing<String> {
    // Windows adb shell的stdin可能按文本读取，裸0x1a会被当作EOF。
    // 仅传无换行ASCII，设备端直接解码为流；编码正文同样不得保存或记录。
    Zeroizing::new(STANDARD.encode(bytes))
}

fn remaining(deadline: Instant, maximum: Duration) -> AppResult<Duration> {
    let value = deadline.saturating_duration_since(Instant::now());
    if value <= Duration::from_secs(2) {
        return Err(AppError::InvalidConfig(
            "操作准备已超时，未发送文字，请刷新后重试".into(),
        ));
    }
    Ok(maximum.min(value - Duration::from_secs(2)))
}
fn adb_shell(ip: &str, command: String) -> Vec<String> {
    vec![
        "-s".into(),
        format!("{ip}:5555"),
        "shell".into(),
        "-T".into(),
        command,
    ]
}
fn correct_hash(output: &[u8], expected: &str) -> bool {
    std::str::from_utf8(output)
        .ok()
        .and_then(|s| s.split_whitespace().next())
        == Some(expected)
}
fn uptime_ms(output: &[u8]) -> AppResult<u64> {
    let first = std::str::from_utf8(output)
        .ok()
        .and_then(|s| s.split_whitespace().next())
        .ok_or_else(|| AppError::InvalidConfig("未能核对设备输入期限，未发送文字".into()))?;
    let (seconds, fraction) = first.split_once('.').unwrap_or((first, "0"));
    if seconds.is_empty()
        || fraction.is_empty()
        || fraction.len() > 3
        || !seconds
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return Err(AppError::InvalidConfig(
            "设备计时结果无效，未发送文字".into(),
        ));
    }
    let millis = seconds
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(1000))
        .and_then(|n| {
            fraction
                .parse::<u64>()
                .ok()
                .and_then(|f| n.checked_add(f * 10u64.pow(3 - fraction.len() as u32)))
        })
        .filter(|n| *n < i64::MAX as u64 - 4000)
        .ok_or_else(|| AppError::InvalidConfig("设备计时结果无效，未发送文字".into()))?;
    Ok(millis)
}
fn packet(action: &ControlAction, issued: u64) -> AppResult<Zeroizing<Vec<u8>>> {
    let (operation, bytes) = match action {
        ControlAction::Text { text } => {
            super::validate_action(action)?;
            (1u8, text.as_bytes())
        }
        ControlAction::Key {
            key: ControlKey::ClearText,
        } => (2u8, &[][..]),
        _ => return Err(AppError::InvalidConfig("不支持的文字操作".into())),
    };
    let mut result = Zeroizing::new(Vec::with_capacity(23 + bytes.len()));
    result.extend_from_slice(&0x494e5831u32.to_be_bytes());
    result.extend_from_slice(&issued.to_be_bytes());
    result.extend_from_slice(&(issued + TTL.as_millis() as u64).to_be_bytes());
    result.push(operation);
    result.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    result.extend_from_slice(bytes);
    Ok(result)
}

#[derive(PartialEq, Eq, Debug)]
enum HelperStatus {
    Applied,
    Rejected,
}
fn status(output: &[u8]) -> AppResult<HelperStatus> {
    let text = std::str::from_utf8(output)
        .map_err(|_| AppError::Conflict("文字操作结果未确认，请刷新屏幕核对".into()))?;
    let markers: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|s| s.starts_with("XX_TEXT_"))
        .collect();
    match markers.as_slice() {
        ["XX_TEXT_APPLIED"] => Ok(HelperStatus::Applied),
        [
            "XX_TEXT_REJECTED"
            | "XX_TEXT_NOT_READY"
            | "XX_TEXT_FOCUS_CHANGED"
            | "XX_TEXT_AMBIGUOUS"
            | "XX_TEXT_EXPIRED",
        ] => Ok(HelperStatus::Rejected),
        _ => Err(AppError::Conflict(
            "文字操作结果未确认，请刷新屏幕核对".into(),
        )),
    }
}

async fn command(
    adb: &Path,
    ip: &str,
    script: String,
    deadline: Instant,
    cancel: CancellationToken,
) -> AppResult<Vec<u8>> {
    super::super::transport::run_bytes(
        adb,
        &adb_shell(ip, script),
        remaining(deadline, Duration::from_secs(3))?,
        cancel,
        4096,
        4096,
    )
    .await
}

pub(super) async fn send(
    adb: &Path,
    ip: &str,
    action: &ControlAction,
    deadline: Instant,
    cancel: CancellationToken,
) -> AppResult<()> {
    let sha = EXPECTED_SHA.trim();
    if sha.len() != 64 || hex::encode(Sha256::digest(JAR)) != sha {
        return Err(AppError::InvalidConfig(
            "文字辅助组件校验失败，请重新安装工作台".into(),
        ));
    }
    let path = format!("/data/local/tmp/inx-remote-text-{sha}.jar");
    let verify = format!("if [ -f '{path}' ]; then sha256sum '{path}'; else printf MISSING; fi");
    let found = command(adb, ip, verify.clone(), deadline, cancel.clone()).await?;
    if !correct_hash(&found, sha) {
        let encoded_jar = ascii_stdin(JAR);
        super::super::transport::run_stdin_output(
            adb,
            &adb_shell(
                ip,
                format!("umask 077; base64 -d > '{path}' && chmod 600 '{path}'"),
            ),
            encoded_jar.as_bytes(),
            remaining(deadline, Duration::from_secs(4))?,
            cancel.clone(),
        )
        .await?;
        let uploaded = command(adb, ip, verify, deadline, cancel.clone()).await?;
        if !correct_hash(&uploaded, sha) {
            return Err(AppError::InvalidConfig(
                "屏端文字辅助组件校验失败，未发送文字".into(),
            ));
        }
    }
    // 先完成无凭据准备，再给正文分配独立且短的设备单调时钟期限。
    if deadline.saturating_duration_since(Instant::now()) < TTL + SETTLE + Duration::from_secs(3) {
        return Err(AppError::InvalidConfig(
            "操作准备已超时，未发送文字，请刷新后重试".into(),
        ));
    }
    let clock = command(adb, ip, "cat /proc/uptime".into(), deadline, cancel.clone()).await?;
    let received = Instant::now();
    let issued = uptime_ms(&clock)?;
    let until_settled = received + TTL + SETTLE;
    if until_settled + Duration::from_secs(1) >= deadline || cancel.is_cancelled() {
        return Err(AppError::InvalidConfig(
            "本次文字操作期限不足，未发送文字，请重试".into(),
        ));
    }
    let body = packet(action, issued)?;
    let encoded_body = ascii_stdin(&body);
    let arguments = adb_shell(
        ip,
        format!(
            "base64 -d | uiautomator runtest '{path}' -c com.inx.remote.FocusedText#testSetFocusedText"
        ),
    );
    // 启动后不因普通停止截断等候；过期helper不能再发起节点动作，未知结果也等待期限收敛。
    let result = super::super::transport::run_stdin_output(
        adb,
        &arguments,
        encoded_body.as_bytes(),
        TTL + Duration::from_secs(2),
        CancellationToken::new(),
    )
    .await;
    let parsed = result.and_then(|bytes| status(&bytes));
    settle_result(parsed, until_settled).await
}

async fn settle_result(parsed: AppResult<HelperStatus>, until_settled: Instant) -> AppResult<()> {
    match parsed {
        Ok(HelperStatus::Applied) => Ok(()),
        Ok(HelperStatus::Rejected) => Err(AppError::InvalidConfig(
            "未修改输入框，请点击屏端输入框后刷新再试".into(),
        )),
        Err(error) => {
            tokio::time::sleep_until(until_settled).await;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adb_stdin_is_ascii_and_roundtrips_control_bytes_and_the_complete_jar() {
        let controls: Vec<u8> = (0u8..=255).collect();
        let request = packet(
            &ControlAction::Text {
                text: "N42_%s'&".into(),
            },
            0x0d0a001a,
        )
        .unwrap();
        for original in [controls.as_slice(), JAR, request.as_slice()] {
            let encoded = ascii_stdin(original);
            assert!(
                encoded
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
            );
            assert!(
                !encoded
                    .bytes()
                    .any(|b| matches!(b, 0 | 0x1a | b'\r' | b'\n'))
            );
            let decoded = STANDARD.decode(encoded.as_bytes()).unwrap();
            assert_eq!(decoded.as_slice(), original);
            assert_eq!(Sha256::digest(&decoded), Sha256::digest(original));
        }
        assert!(JAR.contains(&0x1a), "JAR传输必须覆盖文本EOF控制字节");
    }
    #[test]
    fn embedded_helper_hash_and_strict_result_contract() {
        assert_eq!(hex::encode(Sha256::digest(JAR)), EXPECTED_SHA.trim());
        assert!(JAR.len() < 16 * 1024);
        assert_eq!(
            status(b"OK (1 test)\nXX_TEXT_APPLIED\n").unwrap(),
            HelperStatus::Applied
        );
        assert_eq!(
            status(b"XX_TEXT_EXPIRED\n").unwrap(),
            HelperStatus::Rejected
        );
        for output in [
            b"OK (1 test)".as_slice(),
            b"XX_TEXT_APPLIED\nXX_TEXT_APPLIED",
            b"XX_TEXT_APPLIED secret",
            b"XX_TEXT_FUTURE",
            b"XX_TEXT_ERROR",
            b"\xff",
        ] {
            assert!(status(output).is_err());
        }
    }
    #[test]
    fn protocol_preserves_text_and_distinguishes_clear_without_empty_text() {
        let text = "N42_%s'&";
        let value = packet(&ControlAction::Text { text: text.into() }, 10000).unwrap();
        assert_eq!(&value[..4], b"INX1");
        assert_eq!(u64::from_be_bytes(value[4..12].try_into().unwrap()), 10000);
        assert_eq!(u64::from_be_bytes(value[12..20].try_into().unwrap()), 14000);
        assert_eq!(&value[20..23], &[1, 0, 8]);
        assert_eq!(&value[23..], text.as_bytes());
        let clear = packet(
            &ControlAction::Key {
                key: ControlKey::ClearText,
            },
            10000,
        )
        .unwrap();
        assert_eq!(&clear[20..], &[2, 0, 0]);
        assert!(
            packet(
                &ControlAction::Text {
                    text: String::new()
                },
                10000
            )
            .is_err()
        );
        assert!(
            packet(
                &ControlAction::Key {
                    key: ControlKey::Backspace
                },
                10000
            )
            .is_err()
        );
    }
    #[test]
    fn uptime_is_exact_and_invalid_clocks_cannot_create_deadlines() {
        assert_eq!(uptime_ms(b"123.45 234.0\n").unwrap(), 123450);
        assert_eq!(uptime_ms(b"123 2\n").unwrap(), 123000);
        for value in [
            b"-1.0".as_slice(),
            b"1.9999",
            b"NaN",
            b"1e3",
            b"18446744073709551615.9",
            b"\xff",
        ] {
            assert!(uptime_ms(value).is_err());
        }
    }
    #[tokio::test]
    async fn unknown_result_keeps_the_execution_window_open_without_resending() {
        let began = Instant::now();
        let result = settle_result(
            Err(AppError::timeout("固定测试状态")),
            began + Duration::from_millis(80),
        )
        .await;
        assert!(result.is_err());
        assert!(began.elapsed() >= Duration::from_millis(80));
        let began = Instant::now();
        assert!(
            settle_result(Ok(HelperStatus::Rejected), began + Duration::from_secs(1))
                .await
                .is_err()
        );
        assert!(began.elapsed() < Duration::from_millis(500));
    }
}
