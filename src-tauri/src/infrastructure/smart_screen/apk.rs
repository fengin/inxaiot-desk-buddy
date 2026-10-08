use super::device::{AdbDevice, AndroidTools, XIAOXIN_PACKAGE, find_on_path, run};
use crate::core::error::{AppError, AppResult};
use base64::Engine;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApkInfo {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub last_modified: u64,
    pub sha256: String,
    pub package_id: String,
    pub app_version: String,
    pub app_version_code: u64,
    pub min_sdk: u32,
    pub abis: Vec<String>,
    pub activity: String,
    pub signer_sha256: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct PackageTools {
    pub aapt: PathBuf,
    pub apksigner: PathBuf,
    pub java: PathBuf,
}
fn executable_arg(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(value) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{value}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
    }
}
impl PackageTools {
    pub fn discover(android: &AndroidTools) -> AppResult<Self> {
        let java_name=super::tool_bundle::executable_name("java");
        let aapt_name=super::tool_bundle::executable_name("aapt");
        if let Some(root) = super::tool_bundle::current()? {
            return Ok(Self {
                aapt: root.join("android-build").join(&aapt_name),
                apksigner: root.join("android-build/lib/apksigner.jar"),
                java: root.join("java/bin").join(&java_name),
            });
        }
        let java = std::env::var_os("JAVA_HOME")
            .map(PathBuf::from)
            .map(|p| p.join("bin").join(&java_name))
            .filter(|p| p.is_file())
            .or_else(|| find_on_path(&java_name))
            .ok_or_else(|| AppError::NotFound("未找到安装包签名检查所需的 Java 工具".into()))?;
        if let (Some(aapt), Some(jar)) = (
            std::env::var_os("INX_AAPT_PATH"),
            std::env::var_os("INX_APKSIGNER_JAR"),
        ) {
            let (aapt, apksigner) = (PathBuf::from(aapt), PathBuf::from(jar));
            if aapt.is_file() && apksigner.is_file() {
                return Ok(Self {
                    aapt,
                    apksigner,
                    java,
                });
            }
            return Err(AppError::NotFound("配置的 APK 检查工具不完整".into()));
        }
        let mut roots = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                roots.push(parent.join("tools/android-build"));
            }
        }
        let sdk = android
            .adb
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| AppError::NotFound("Android SDK 路径无效".into()))?;
        if let Ok(entries) = std::fs::read_dir(sdk.join("build-tools")) {
            let mut versions = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect::<Vec<_>>();
            versions.sort_by_key(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .split('.')
                    .map(|n| n.parse::<u32>().unwrap_or(0))
                    .collect::<Vec<_>>()
            });
            versions.reverse();
            roots.extend(versions);
        }
        for root in roots {
            let aapt = root.join(&aapt_name);
            let apksigner = root.join("lib/apksigner.jar");
            if aapt.is_file() && apksigner.is_file() {
                return Ok(Self {
                    aapt,
                    apksigner,
                    java,
                });
            }
        }
        Err(AppError::NotFound(
            "未找到完整的 Android APK 检查工具（aapt、apksigner）".into(),
        ))
    }
    pub async fn signers(&self, path: &Path, cancel: CancellationToken) -> AppResult<Vec<String>> {
        let output = run(
            &self.java,
            &[
                "-jar".into(),
                executable_arg(&self.apksigner),
                "verify".into(),
                "--print-certs".into(),
                executable_arg(path),
            ],
            Duration::from_secs(60),
            cancel,
        )
        .await?;
        if !output.success {
            return Err(AppError::Integrity {
                operation: "APK 签名校验失败，安装包可能损坏或不完整",
            });
        }
        let expression =
            Regex::new(r"(?m)^Signer #\d+ certificate SHA-256 digest: ([0-9a-fA-F]{64})\s*$")
                .unwrap();
        let mut signers = expression
            .captures_iter(&output.stdout)
            .map(|c| c[1].to_ascii_lowercase())
            .collect::<Vec<_>>();
        signers.sort();
        signers.dedup();
        if signers.is_empty() {
            return Err(AppError::InvalidConfig(
                "未取得安装包签名，不能继续安装".into(),
            ));
        }
        Ok(signers)
    }
}
pub async fn sha256(path: &Path) -> AppResult<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| AppError::io("打开 APK 文件", &e))?;
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut hash = Sha256::new();
    loop {
        let count = file
            .read(&mut buffer)
            .await
            .map_err(|e| AppError::io("读取 APK 文件", &e))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex::encode(hash.finalize()))
}
pub fn parse_badging(text: &str) -> AppResult<(String, String, u64, u32, Vec<String>, String)> {
    let capture = |expression: &str| {
        Regex::new(expression)
            .unwrap()
            .captures(text)
            .map(|c| c[1].to_string())
    };
    let package = capture(r"(?m)^package: name='([^']+)'")
        .ok_or_else(|| AppError::InvalidConfig("无法读取 APK 包名".into()))?;
    if package != XIAOXIN_PACKAGE {
        return Err(AppError::InvalidConfig(
            "首期只支持智能小新应用安装包".into(),
        ));
    }
    let version = capture(r"(?m)^package:.* versionName='([^']*)'")
        .filter(|v| !v.is_empty() && v.chars().count() <= 32)
        .ok_or_else(|| AppError::InvalidConfig("APK 版本名称无效或超过 32 字符".into()))?;
    let code = capture(r"(?m)^package:.* versionCode='(\d+)'")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| AppError::InvalidConfig("无法读取 APK 数字版本号".into()))?;
    let sdk = capture(r"(?m)^sdkVersion:'(\d+)'")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| AppError::InvalidConfig("无法读取 APK 最低系统要求".into()))?;
    let activity = capture(r"(?m)^launchable-activity: name='([^']+)'")
        .filter(|v| {
            v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
        })
        .ok_or_else(|| AppError::InvalidConfig("未找到有效的小新启动页面".into()))?;
    let native = text
        .lines()
        .find(|l| l.starts_with("native-code:"))
        .unwrap_or("");
    let abis = Regex::new(r"'([^']+)'")
        .unwrap()
        .captures_iter(native)
        .map(|c| c[1].to_string())
        .collect();
    Ok((package, version, code, sdk, abis, activity))
}
pub async fn inspect(path: &Path, cancel: CancellationToken) -> AppResult<ApkInfo> {
    if !path
        .extension()
        .is_some_and(|v| v.eq_ignore_ascii_case("apk"))
    {
        return Err(AppError::InvalidConfig("请选择 APK 文件".into()));
    }
    let canonical = tokio::fs::canonicalize(path)
        .await
        .map_err(|e| AppError::io("读取安装包路径", &e))?;
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|e| AppError::io("读取安装包信息", &e))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 2 * 1024 * 1024 * 1024 {
        return Err(AppError::InvalidConfig(
            "安装包必须是非空、2 GB 以内的文件".into(),
        ));
    }
    let hash = sha256(&canonical).await?;
    let tools = PackageTools::discover(&AndroidTools::discover()?)?;
    let output = run(
        &tools.aapt,
        &["dump".into(), "badging".into(), executable_arg(&canonical)],
        Duration::from_secs(60),
        cancel.clone(),
    )
    .await?;
    if !output.success {
        return Err(AppError::InvalidConfig(
            "无法解析 APK，请确认安装包完整".into(),
        ));
    }
    let (package_id, app_version, app_version_code, min_sdk, abis, activity) =
        parse_badging(&output.stdout)?;
    let signer_sha256 = tools.signers(&canonical, cancel).await?;
    if sha256(&canonical).await? != hash {
        return Err(AppError::Conflict(
            "解析期间安装包发生变化，请重新选择".into(),
        ));
    }
    Ok(ApkInfo {
        name: canonical
            .file_name()
            .ok_or_else(|| AppError::InvalidConfig("安装包文件名无效".into()))?
            .to_string_lossy()
            .into_owned(),
        path: executable_arg(&canonical),
        size: metadata.len(),
        last_modified: metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        sha256: hash,
        package_id,
        app_version,
        app_version_code,
        min_sdk,
        abis,
        activity,
        signer_sha256,
    })
}
fn supported_installed_apk_path(path: &str) -> bool {
    // 厂家预装小新位于只读系统/OEM目录；升级签名核对仍只读取 pm path 返回的本应用包。
    const ROOTS: &[&str] = &[
        "/data/app/", "/system/app/", "/system/priv-app/", "/system_ext/app/",
        "/system_ext/priv-app/", "/product/app/", "/product/priv-app/",
        "/vendor/app/", "/vendor/priv-app/", "/oem/app/", "/oem/priv-app/",
        "/oem/bundled_persist-app/",
    ];
    ROOTS.iter().any(|root| path.starts_with(root))
        && path.ends_with(".apk")
        && path.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"/._-+=@~".contains(&byte))
        && !path.split('/').any(|part| part == "." || part == "..")
}

pub async fn installed_signers(
    device: &AdbDevice,
    ip: &str,
    identity: &str,
    directory: &Path,
    cancel: CancellationToken,
) -> AppResult<Option<Vec<String>>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<String, Vec<String>>>,
    > = std::sync::OnceLock::new();
    let paths = device
        .shell(ip, &["pm", "path", XIAOXIN_PACKAGE], cancel.clone())
        .await?;
    let Some(remote) = paths
        .lines()
        .find_map(|line| line.strip_prefix("package:"))
        .filter(|path| supported_installed_apk_path(path))
    else {
        if paths.is_empty() {
            return Ok(None);
        }
        return Err(AppError::Conflict("无法确认已安装小新包的来源路径".into()));
    };
    let stamp = device
        .shell(ip, &["stat", "-c", "%s:%Y", remote], cancel.clone())
        .await?;
    if !Regex::new(r"^\d+:\d+$").unwrap().is_match(stamp.trim()) {
        return Err(AppError::Conflict("无法确认已安装 APK 文件状态".into()));
    }
    let key = format!("{ip}|{identity}|{remote}|{}", stamp.trim());
    if let Some(signers) = CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| AppError::Conflict("签名缓存暂不可用".into()))?
        .get(&key)
        .cloned()
    {
        return Ok(Some(signers));
    }
    // 已安装包由 Android 完成安装校验；从 APK v2 签名区读取证书即可比对签名。
    // 用户选择的待安装包仍经 apksigner 完整校验，不以签名区解析代替包完整性校验。
    let length = stamp
        .split(':')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| AppError::Conflict("已安装包大小无效".into()))?;
    if let Some(signers) = installed_v2_signers(device, ip, remote, length, cancel.clone()).await? {
        if device
            .shell(ip, &["pm", "path", XIAOXIN_PACKAGE], cancel.clone())
            .await?
            != paths
            || device
                .shell(ip, &["stat", "-c", "%s:%Y", remote], cancel.clone())
                .await?
                != stamp
        {
            return Err(AppError::Conflict(
                "签名核对期间应用发生变化，请重新检查".into(),
            ));
        }
        CACHE
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| AppError::Conflict("签名缓存暂不可用".into()))?
            .insert(key, signers.clone());
        return Ok(Some(signers));
    }
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|e| AppError::io("准备签名核对目录", &e))?;
    let local = directory.join(format!("installed-{}.apk", uuid::Uuid::now_v7()));
    let result = device
        .adb(
            ip,
            &["pull", remote, &executable_arg(&local)],
            Duration::from_secs(1800),
            cancel.clone(),
        )
        .await;
    if result.as_ref().is_err() || result.as_ref().is_ok_and(|r| !r.success) {
        let _ = tokio::fs::remove_file(&local).await;
        return Err(result.err().unwrap_or_else(|| {
            AppError::Conflict("读取已安装小新包失败，无法核对保留数据升级的签名".into())
        }));
    }
    let checked = PackageTools::discover(&device.tools)?
        .signers(&local, cancel)
        .await;
    let _ = tokio::fs::remove_file(&local).await;
    let signers = checked?;
    let current_path = device
        .shell(
            ip,
            &["pm", "path", XIAOXIN_PACKAGE],
            CancellationToken::new(),
        )
        .await?;
    let current_stamp = device
        .shell(
            ip,
            &["stat", "-c", "%s:%Y", remote],
            CancellationToken::new(),
        )
        .await?;
    if current_path != paths || current_stamp != stamp {
        return Err(AppError::Conflict(
            "签名核对期间应用发生变化，请重新检查".into(),
        ));
    }
    CACHE
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| AppError::Conflict("签名缓存暂不可用".into()))?
        .insert(key, signers.clone());
    Ok(Some(signers))
}
fn invalid_signing_block() -> AppError {
    AppError::Conflict("已安装包的签名区格式不完整，不能确认保留数据安装条件".into())
}
async fn remote_range(
    device: &AdbDevice,
    ip: &str,
    remote: &str,
    offset: u64,
    length: usize,
    cancel: CancellationToken,
) -> AppResult<Vec<u8>> {
    if length > 4 * 1024 * 1024 || !supported_installed_apk_path(remote) {
        return Err(invalid_signing_block());
    }
    let start = offset / 4096;
    let remainder = (offset % 4096) as usize;
    let blocks = (remainder + length).div_ceil(4096);
    let command =
        format!("dd if='{remote}' bs=4096 skip={start} count={blocks} 2>/dev/null | base64");
    let text = device.shell(ip, &[&command], cancel).await?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.split_whitespace().collect::<String>())
        .map_err(|_| invalid_signing_block())?;
    bytes
        .get(remainder..remainder + length)
        .map(|s| s.to_vec())
        .ok_or_else(invalid_signing_block)
}
fn part<'a>(bytes: &mut &'a [u8]) -> AppResult<&'a [u8]> {
    let prefix = bytes.get(..4).ok_or_else(invalid_signing_block)?;
    let size = u32::from_le_bytes(prefix.try_into().unwrap()) as usize;
    let value = bytes
        .get(4..4usize.checked_add(size).ok_or_else(invalid_signing_block)?)
        .ok_or_else(invalid_signing_block)?;
    *bytes = &bytes[4 + size..];
    Ok(value)
}
pub fn v2_certificates(block: &[u8]) -> AppResult<Vec<String>> {
    let mut content = block;
    let mut signers = part(&mut content)?;
    if !content.is_empty() {
        return Err(invalid_signing_block());
    }
    let mut certificates = Vec::new();
    while !signers.is_empty() {
        let mut signer = part(&mut signers)?;
        let mut signed = part(&mut signer)?;
        let _digests = part(&mut signed)?;
        let mut chain = part(&mut signed)?;
        let certificate = part(&mut chain)?;
        if certificate.first() != Some(&0x30) {
            return Err(invalid_signing_block());
        }
        certificates.push(hex::encode(Sha256::digest(certificate)));
    }
    if certificates.is_empty() {
        return Err(invalid_signing_block());
    }
    certificates.sort();
    certificates.dedup();
    Ok(certificates)
}
async fn installed_v2_signers(
    device: &AdbDevice,
    ip: &str,
    remote: &str,
    size: u64,
    cancel: CancellationToken,
) -> AppResult<Option<Vec<String>>> {
    if size < 22 {
        return Err(invalid_signing_block());
    }
    let tail_size = size.min(65557) as usize;
    let tail = remote_range(
        device,
        ip,
        remote,
        size - tail_size as u64,
        tail_size,
        cancel.clone(),
    )
    .await?;
    let end = (0..=tail.len() - 22)
        .rev()
        .find(|&i| {
            tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06]
                && i + 22 + u16::from_le_bytes([tail[i + 20], tail[i + 21]]) as usize == tail.len()
        })
        .ok_or_else(invalid_signing_block)?;
    let directory = u32::from_le_bytes(tail[end + 16..end + 20].try_into().unwrap()) as u64;
    if directory < 24 || directory > size - tail_size as u64 + end as u64 {
        return Err(invalid_signing_block());
    }
    let footer = remote_range(device, ip, remote, directory - 24, 24, cancel.clone()).await?;
    if &footer[8..] != b"APK Sig Block 42" {
        return Ok(None);
    }
    let length = u64::from_le_bytes(footer[..8].try_into().unwrap());
    if length < 24 || length > 4 * 1024 * 1024 - 8 || length + 8 > directory {
        return Err(invalid_signing_block());
    }
    let block = remote_range(
        device,
        ip,
        remote,
        directory - length - 8,
        (length + 8) as usize,
        cancel,
    )
    .await?;
    if block[..8] != footer[..8] {
        return Err(invalid_signing_block());
    }
    let mut pairs = &block[8..block.len() - 24];
    while !pairs.is_empty() {
        let prefix = pairs.get(..8).ok_or_else(invalid_signing_block)?;
        let size = u64::from_le_bytes(prefix.try_into().unwrap());
        if size < 4 || size > pairs.len().saturating_sub(8) as u64 {
            return Err(invalid_signing_block());
        }
        let id = u32::from_le_bytes(pairs[8..12].try_into().unwrap());
        if id == 0x7109871a {
            return Ok(Some(v2_certificates(&pairs[12..8 + size as usize])?));
        }
        pairs = &pairs[8 + size as usize..];
    }
    Ok(None)
}
pub async fn stage(info: &ApkInfo, directory: &Path) -> AppResult<ApkInfo> {
    if sha256(Path::new(&info.path)).await? != info.sha256 {
        return Err(AppError::Conflict("安装包已改变，请重新选择并检查".into()));
    }
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|e| AppError::io("创建安装任务目录", &e))?;
    let destination = directory.join("xiaoxin.apk");
    tokio::fs::copy(&info.path, &destination)
        .await
        .map_err(|e| AppError::io("准备本次安装包", &e))?;
    if sha256(&destination).await? != info.sha256 {
        return Err(AppError::Integrity {
            operation: "本次安装包复制校验",
        });
    }
    let mut staged = info.clone();
    staged.path = executable_arg(&destination);
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_package_path_accepts_factory_apks_without_allowing_shell_or_path_escape() {
        for path in [
            "/data/app/~~abc+12==/chat.xiaoxin.app-Ab_12==/base.apk",
            "/system/priv-app/Xiaoxin/Xiaoxin.apk",
            "/product/app/Xiaoxin/base.apk",
            "/oem/bundled_persist-app/xiaoxin-2.0.2-armeabi-v7a-release/xiaoxin-2.0.2-armeabi-v7a-release.apk",
        ] {
            assert!(supported_installed_apk_path(path), "应支持已安装包路径：{path}");
        }
        for path in [
            "/sdcard/xiaoxin.apk", "/data/app/../../sdcard/xiaoxin.apk", "/data/app/./base.apk",
            "/data/app/base.apk;reboot.apk", "/oem/app/$(reboot).apk", "/data/app/a`reboot`.apk",
            "/data/app/a\nb.apk", "/data/app/a b.apk", "/data/app/base.zip",
        ] {
            assert!(!supported_installed_apk_path(path), "应拒绝不可信路径：{path}");
        }
    }
    #[test]
    fn reads_package_metadata_and_rejects_other_app_and_missing_version() {
        let text = "package: name='chat.xiaoxin.app' versionCode='7019' versionName='2.0.9'\nsdkVersion:'24'\nlaunchable-activity: name='chat.xiaoxin.app.MainActivity'\nnative-code: 'arm64-v8a'\n";
        let value = parse_badging(text).unwrap();
        assert_eq!(value.1, "2.0.9");
        assert_eq!(value.2, 7019);
        assert_eq!(value.3, 24);
        assert_eq!(value.4, vec!["arm64-v8a"]);
        assert!(parse_badging(&text.replace("chat.xiaoxin.app", "other.app")).is_err());
        assert!(parse_badging(&text.replace("versionCode='7019'", "")).is_err());
    }
    #[test]
    fn installed_v2_certificate_parser_rejects_truncation_and_hashes_leaf_certificate() {
        fn sized(value: &[u8]) -> Vec<u8> {
            let mut bytes = (value.len() as u32).to_le_bytes().to_vec();
            bytes.extend(value);
            bytes
        }
        let cert = [0x30, 0x01, 0x42];
        let mut signed = sized(&[]);
        signed.extend(sized(&sized(&cert)));
        signed.extend(sized(&[]));
        let mut signer = sized(&signed);
        signer.extend(sized(&[]));
        signer.extend(sized(&[]));
        let block = sized(&sized(&signer));
        assert_eq!(
            v2_certificates(&block).unwrap(),
            vec![hex::encode(Sha256::digest(cert))]
        );
        for length in 0..block.len() {
            assert!(v2_certificates(&block[..length]).is_err());
        }
    }
}
