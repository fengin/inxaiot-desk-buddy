//! 只读取 APK 内必要的文件；不调用 Android SDK 或 Java，不执行 APK 内容。
use super::device::XIAOXIN_PACKAGE;
use crate::core::error::{AppError, AppResult};
use apk_info_axml::{ARSC, AXML};
use apk_info_xml::Element;
use std::{collections::BTreeSet, io::Read, path::Path};
use tokio_util::sync::CancellationToken;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Metadata {
    pub package: String,
    pub version: String,
    pub code: u64,
    pub sdk: u32,
    pub abis: Vec<String>,
    pub activity: String,
}
fn invalid(message: &str) -> AppError {
    AppError::InvalidConfig(message.into())
}

fn read_entry(
    archive: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
    limit: u64,
) -> AppResult<Vec<u8>> {
    let mut entry = archive
        .by_name(name)
        .map_err(|_| invalid("APK 缺少必要的清单或资源文件"))?;
    if entry.is_dir() || entry.size() == 0 || entry.size() > limit {
        return Err(invalid("APK 清单或资源文件大小无效"));
    }
    let declared = entry.size();
    let mut bytes = Vec::new();
    (&mut entry)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("APK 内容无法读取，请确认安装包完整"))?;
    if bytes.len() as u64 != declared {
        return Err(invalid("APK 内容长度与记录不一致"));
    }
    Ok(bytes)
}

fn validate_binary_manifest(bytes: &[u8]) -> AppResult<()> {
    let bad = || invalid("APK 清单格式不完整，无法读取安装包信息");
    if bytes.len() < 8
        || bytes[..4] != [3, 0, 8, 0]
        || u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize != bytes.len()
    {
        return Err(bad());
    }
    let (mut offset, mut depth, mut elements) = (8usize, 0usize, 0usize);
    while offset < bytes.len() {
        let header = bytes.get(offset..offset + 8).ok_or_else(bad)?;
        let kind = u16::from_le_bytes(header[..2].try_into().unwrap());
        let head_size = u16::from_le_bytes(header[2..4].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        if head_size < 8 || size < head_size || size > bytes.len() - offset {
            return Err(bad());
        }
        if kind == 0x0102 {
            depth += 1;
            elements += 1;
            if depth > 64 || elements > 20000 {
                return Err(bad());
            }
        }
        if kind == 0x0103 {
            depth = depth.checked_sub(1).ok_or_else(bad)?;
        }
        offset += size;
    }
    if depth != 0 || elements == 0 {
        return Err(bad());
    }
    Ok(())
}
fn number(value: &str) -> AppResult<u64> {
    value
        .strip_prefix("0x")
        .map(|v| u64::from_str_radix(v, 16))
        .unwrap_or_else(|| value.parse())
        .map_err(|_| invalid("APK 版本号或最低系统版本无效"))
}

fn metadata(
    root: &Element,
    resolve: impl Fn(&str, &str) -> Option<String>,
    abis: Vec<String>,
) -> AppResult<Metadata> {
    if root.name() != "manifest" {
        return Err(invalid("APK 缺少应用清单"));
    }
    let package = root.attr("package").unwrap_or_default().to_string();
    if package != XIAOXIN_PACKAGE {
        return Err(invalid("首期只支持智能小新应用安装包"));
    }
    if root.attr("split").is_some_and(|s| !s.is_empty()) {
        return Err(invalid(
            "请选择可独立安装的小新 APK，不能使用拆分安装中的单个配置包",
        ));
    }
    let version =
        resolve("manifest", "versionName").ok_or_else(|| invalid("无法读取 APK 版本名称"))?;
    if version.is_empty() || version.starts_with('@') || version.chars().count() > 32 {
        return Err(invalid("APK 版本名称无效或超过 32 字符"));
    }
    let low = number(
        &resolve("manifest", "versionCode").ok_or_else(|| invalid("无法读取 APK 数字版本号"))?,
    )?;
    let high = resolve("manifest", "versionCodeMajor")
        .map(|s| number(&s))
        .transpose()?
        .unwrap_or(0);
    if low > u32::MAX as u64 || high > i32::MAX as u64 {
        return Err(invalid("APK 数字版本号超出 Android 支持范围"));
    }
    let sdk = resolve("uses-sdk", "minSdkVersion")
        .map(|s| number(&s))
        .transpose()?
        .unwrap_or(1);
    let sdk = u32::try_from(sdk)
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid("APK 最低系统要求无效"))?;
    let app = root
        .childrens()
        .find(|e| e.name() == "application")
        .ok_or_else(|| invalid("APK 缺少应用声明"))?;
    if app.attr("enabled") == Some("false") {
        return Err(invalid("小新应用在安装包中被禁用"));
    }
    let mut activities = BTreeSet::new();
    for node in app
        .childrens()
        .filter(|e| matches!(e.name(), "activity" | "activity-alias"))
    {
        if node.attr("enabled") == Some("false") || node.attr("exported") == Some("false") {
            continue;
        }
        let launcher = node
            .childrens()
            .filter(|e| e.name() == "intent-filter")
            .any(|filter| {
                filter.childrens().any(|e| {
                    e.name() == "action" && e.attr("name") == Some("android.intent.action.MAIN")
                }) && filter.childrens().any(|e| {
                    e.name() == "category"
                        && e.attr("name") == Some("android.intent.category.LAUNCHER")
                })
            });
        if !launcher {
            continue;
        }
        let name = node.attr("name").unwrap_or_default();
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._".contains(&b))
        {
            return Err(invalid("小新启动页面名称无效"));
        }
        activities.insert(if name.starts_with('.') {
            format!("{package}{name}")
        } else if name.contains('.') {
            name.into()
        } else {
            format!("{package}.{name}")
        });
    }
    if activities.len() != 1 {
        return Err(invalid("未找到唯一可启动的小新页面"));
    }
    Ok(Metadata {
        package,
        version,
        code: (high << 32) | low,
        sdk,
        abis,
        activity: activities.pop_first().unwrap(),
    })
}

pub(super) fn read(path: &Path, cancel: &CancellationToken) -> AppResult<Metadata> {
    let file = std::fs::File::open(path).map_err(|e| AppError::io("读取 APK 文件", &e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|_| invalid("无法读取 APK，请确认文件是完整的安装包"))?;
    if archive.len() > 100_000 {
        return Err(invalid("APK 文件条目过多"));
    }
    let mut names = BTreeSet::new();
    let mut abis = BTreeSet::new();
    for index in 0..archive.len() {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let entry = archive
            .by_index_raw(index)
            .map_err(|_| invalid("APK 文件目录损坏"))?;
        if !names.insert(entry.name().to_string()) {
            return Err(invalid("APK 包含重复文件，无法确认应用信息"));
        }
        let parts = entry.name().split('/').collect::<Vec<_>>();
        if parts.len() == 3 && parts[0] == "lib" && parts[2].ends_with(".so") && !entry.is_dir() {
            if ![
                "arm64-v8a",
                "armeabi-v7a",
                "armeabi",
                "x86_64",
                "x86",
                "mips",
                "mips64",
                "riscv64",
            ]
            .contains(&parts[1])
            {
                return Err(invalid("APK 包含无法识别的处理器架构"));
            }
            abis.insert(parts[1].to_string());
        }
    }
    let bytes = read_entry(&mut archive, "AndroidManifest.xml", 4 * 1024 * 1024)?;
    validate_binary_manifest(&bytes)?;
    let manifest =
        AXML::new(&mut bytes.as_slice(), None).map_err(|_| invalid("无法解析 APK 应用清单"))?;
    let resource_needed = [
        ("manifest", "versionName"),
        ("manifest", "versionCode"),
        ("uses-sdk", "minSdkVersion"),
    ]
    .iter()
    .any(|(tag, attr)| {
        manifest
            .get_attribute_value(tag, attr, None)
            .is_some_and(|v| v.starts_with('@'))
    });
    let resources = if resource_needed {
        let data = read_entry(&mut archive, "resources.arsc", 32 * 1024 * 1024)?;
        Some(
            ARSC::new(&mut data.as_slice())
                .map_err(|_| invalid("无法解析 APK 版本信息引用的资源"))?,
        )
    } else {
        None
    };
    metadata(
        &manifest.root,
        |tag, attr| manifest.get_attribute_value(tag, attr, resources.as_ref()),
        abis.into_iter().collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest(package: &str, activity: &str) -> Element {
        let mut root = Element::new("manifest");
        root.set_attribute("package", package);
        let mut application = Element::new("application");
        let mut node = Element::new("activity");
        node.set_attribute("name", activity);
        let mut filter = Element::new("intent-filter");
        for (tag, name) in [
            ("action", "android.intent.action.MAIN"),
            ("category", "android.intent.category.LAUNCHER"),
        ] {
            let mut child = Element::new(tag);
            child.set_attribute("name", name);
            filter.append_child(child);
        }
        node.append_child(filter);
        application.append_child(node);
        root.append_child(application);
        root
    }
    fn values(_: &str, attr: &str) -> Option<String> {
        match attr {
            "versionName" => Some("2.0.10".into()),
            "versionCode" => Some("5022".into()),
            "minSdkVersion" => Some("24".into()),
            _ => None,
        }
    }
    #[test]
    fn version_and_launcher_are_read_without_external_tools() {
        let info = metadata(
            &manifest(XIAOXIN_PACKAGE, ".MainActivity"),
            values,
            vec!["arm64-v8a".into()],
        )
        .unwrap();
        assert_eq!(info.version, "2.0.10");
        assert_eq!(info.code, 5022);
        assert_eq!(info.sdk, 24);
        assert_eq!(info.activity, "chat.xiaoxin.app.MainActivity");
        assert!(metadata(&manifest("other.app", ".MainActivity"), values, vec![]).is_err());
        assert!(metadata(&manifest(XIAOXIN_PACKAGE, "bad;name"), values, vec![]).is_err());
        assert!(
            metadata(
                &manifest(XIAOXIN_PACKAGE, "MainActivity"),
                |_, _| None,
                vec![]
            )
            .is_err()
        );
    }
    #[test]
    fn truncated_and_unbalanced_binary_xml_is_rejected() {
        for value in [
            vec![],
            vec![3, 0, 8, 0, 9, 0, 0, 0],
            vec![3, 0, 8, 0, 16, 0, 0, 0, 2, 1, 8, 0, 8, 0, 0, 0],
        ] {
            assert!(validate_binary_manifest(&value).is_err());
        }
    }
}
