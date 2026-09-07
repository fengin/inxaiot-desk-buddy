use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::DeploymentImageInput;

const MAX_IMAGE_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_IMAGE_ARCHIVE_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const MAX_IMAGE_TAR_ENTRIES: usize = 100_000;
const MAX_IMAGE_DECLARED_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const MAX_RELEASE_VERSION_BYTES: usize = 128;

/// 工作台自动生成并交给 Agent 的内部清单。它不是用户发布物契约。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub version: String,
    pub images: Vec<ReleaseImage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseImage {
    pub service: String,
    pub image: String,
    pub archive: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseRuntime {
    pub os: String,
    pub arch: String,
    pub docker: String,
    pub compose: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageArchiveInfo {
    pub path: String,
    pub size: u64,
    pub repo_tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectedDeploymentImages {
    pub image_files: Vec<DeploymentImageInput>,
    pub fingerprint: String,
}

pub fn standard_runtime() -> ReleaseRuntime {
    ReleaseRuntime {
        os: "linux".into(),
        arch: "x86_64".into(),
        docker: ">=20.10".into(),
        compose: ">=2.0".into(),
    }
}

/// 在任务提交前逐个打开镜像归档并核对 RepoTag，避免上传后才发现选错文件。
pub fn inspect_deployment_images(
    images: &[DeploymentImageInput],
) -> AppResult<InspectedDeploymentImages> {
    if images.is_empty() {
        return Err(AppError::InvalidConfig("请至少选择一个服务镜像".into()));
    }
    let mut inspected = Vec::with_capacity(images.len());
    let mut services = BTreeSet::new();
    for image in images {
        let service = image.service_name.trim();
        let tag = image.image_tag.trim();
        if tag.is_empty()
            || tag.chars().any(char::is_whitespace)
            || tag.chars().any(char::is_control)
        {
            return Err(AppError::InvalidConfig(format!(
                "服务 {service} 的镜像标签无效"
            )));
        }
        if !services.insert(service.to_string()) {
            return Err(AppError::InvalidConfig(format!(
                "服务 {service} 重复选择了镜像"
            )));
        }
        let archive = inspect_image_archive(Path::new(image.file_path.trim()))?;
        if !archive.repo_tags.is_empty()
            && !archive.repo_tags.iter().any(|candidate| candidate == tag)
        {
            return Err(AppError::InvalidConfig(format!(
                "服务 {service} 填写的镜像标签 {tag} 不在所选归档的RepoTags中"
            )));
        }
        let file_sha256 = sha256_file(Path::new(&archive.path))?;
        inspected.push((
            DeploymentImageInput {
                service_name: service.to_string(),
                file_path: archive.path,
                image_tag: tag.to_string(),
            },
            file_sha256,
        ));
    }
    inspected.sort_by(|left, right| left.0.service_name.cmp(&right.0.service_name));
    let mut digest = Sha256::new();
    for (image, file_sha256) in &inspected {
        digest.update(image.service_name.as_bytes());
        digest.update([0]);
        digest.update(image.image_tag.as_bytes());
        digest.update([0]);
        digest.update(file_sha256.as_bytes());
        digest.update([0]);
    }
    Ok(InspectedDeploymentImages {
        image_files: inspected.into_iter().map(|(image, _)| image).collect(),
        fingerprint: hex::encode(digest.finalize()),
    })
}

pub fn inspect_image_archive(path: &Path) -> AppResult<ImageArchiveInfo> {
    inspect_image_archive_with_limits(
        path,
        MAX_IMAGE_ARCHIVE_BYTES,
        MAX_IMAGE_TAR_ENTRIES,
        MAX_IMAGE_DECLARED_BYTES,
    )
}

fn inspect_image_archive_with_limits(
    path: &Path,
    max_archive_bytes: u64,
    max_entries: usize,
    max_declared_bytes: u64,
) -> AppResult<ImageArchiveInfo> {
    let canonical = path
        .canonicalize()
        .map_err(|error| AppError::io("读取Docker镜像归档", &error))?;
    let metadata =
        std::fs::metadata(&canonical).map_err(|error| AppError::io("读取镜像属性", &error))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > max_archive_bytes {
        return Err(AppError::InvalidConfig(
            "Docker镜像归档为空、不是文件或超过20GiB安全上限".into(),
        ));
    }
    let file = std::fs::File::open(&canonical)
        .map_err(|error| AppError::io("打开Docker镜像归档", &error))?;
    let mut archive = tar::Archive::new(file);
    let entries = archive
        .entries()
        .map_err(|error| AppError::io("读取Docker镜像tar目录", &error))?;
    let mut entry_count = 0_usize;
    let mut declared_bytes = 0_u64;
    let mut manifest_bytes = None;
    for entry in entries {
        let mut entry = entry.map_err(|error| AppError::io("读取Docker镜像tar项", &error))?;
        entry_count = entry_count.saturating_add(1);
        if entry_count > max_entries {
            return Err(AppError::InvalidConfig(
                "Docker镜像tar条目数超过100000个安全上限".into(),
            ));
        }
        declared_bytes = declared_bytes.saturating_add(entry.size());
        if declared_bytes > max_declared_bytes {
            return Err(AppError::InvalidConfig(
                "Docker镜像tar声明总大小超过20GiB安全上限".into(),
            ));
        }
        let entry_path = entry
            .path()
            .map_err(|error| AppError::io("解析Docker镜像tar路径", &error))?;
        if entry_path.as_ref() != Path::new("manifest.json") {
            continue;
        }
        if entry.size() > MAX_IMAGE_MANIFEST_BYTES {
            return Err(AppError::InvalidConfig(
                "Docker镜像manifest.json超过1MiB".into(),
            ));
        }
        if manifest_bytes.is_some() {
            return Err(AppError::InvalidConfig(
                "Docker镜像归档包含重复manifest.json".into(),
            ));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| AppError::io("读取Docker镜像manifest.json", &error))?;
        manifest_bytes = Some(bytes);
    }
    let bytes = manifest_bytes
        .ok_or_else(|| AppError::InvalidConfig("Docker镜像归档缺少manifest.json".into()))?;
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct DockerManifestEntry {
        #[serde(default)]
        repo_tags: Option<Vec<String>>,
    }
    let records = serde_json::from_slice::<Vec<DockerManifestEntry>>(&bytes)
        .map_err(|_| AppError::InvalidConfig("Docker镜像manifest.json格式无效".into()))?;
    if records.len() != 1 {
        return Err(AppError::InvalidConfig(
            "只能选择单镜像Tar，请重新导出镜像".into(),
        ));
    }
    let repo_tags = records
        .into_iter()
        .flat_map(|record| record.repo_tags.unwrap_or_default())
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty() && tag != "<none>:<none>")
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    Ok(ImageArchiveInfo {
        path: canonical.to_string_lossy().into_owned(),
        size: metadata.len(),
        repo_tags,
    })
}

pub fn runtime_version_satisfies(actual_output: &str, requirement: &str) -> AppResult<bool> {
    let required = parse_runtime_requirement(requirement)?;
    let actual = extract_runtime_version(actual_output)
        .ok_or_else(|| AppError::InvalidConfig("远端运行时版本输出无法解析".into()))?;
    Ok(actual >= required)
}

pub fn sha256_file(path: &Path) -> AppResult<String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| AppError::io("读取待校验文件", &error))?;
    let metadata = std::fs::metadata(&canonical)
        .map_err(|error| AppError::io("读取待校验文件属性", &error))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(AppError::InvalidConfig("待校验文件为空或不是文件".into()));
    }
    let mut file =
        std::fs::File::open(&canonical).map_err(|error| AppError::io("打开待校验文件", &error))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| AppError::io("计算文件SHA-256", &error))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

pub fn validate_release_version(version: &str) -> AppResult<()> {
    let bytes = version.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_RELEASE_VERSION_BYTES
        || matches!(version, "." | "..")
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes
            .iter()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'.' | b'_' | b'-'))
    {
        return Err(AppError::InvalidConfig(
            "内部发布标识只能由字母、数字、点、下划线和连字符组成".into(),
        ));
    }
    Ok(())
}

fn parse_runtime_requirement(value: &str) -> AppResult<(u64, u64, u64)> {
    let version = value.trim().strip_prefix(">=").ok_or_else(|| {
        AppError::InvalidConfig("运行时版本约束必须使用>=主版本.次版本格式".into())
    })?;
    parse_version_triplet(version)
        .ok_or_else(|| AppError::InvalidConfig("运行时版本约束必须使用纯数字版本".into()))
}

fn extract_runtime_version(value: &str) -> Option<(u64, u64, u64)> {
    value
        .split(|character: char| !(character.is_ascii_digit() || character == '.'))
        .filter(|candidate| candidate.contains('.'))
        .find_map(parse_version_triplet)
}

fn parse_version_triplet(value: &str) -> Option<(u64, u64, u64)> {
    if value.is_empty()
        || value.starts_with('.')
        || value.ends_with('.')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    let parts = value.split('.').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts.get(2).map_or(Some(0), |part| part.parse().ok())?,
    ))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{inspect_deployment_images, inspect_image_archive_with_limits};
    use crate::domain::aio::deployment::DeploymentImageInput;

    fn image_tar_manifest(path: &std::path::Path, manifest: serde_json::Value) {
        let file = std::fs::File::create(path).expect("image tar");
        let mut builder = tar::Builder::new(file);
        let content = serde_json::to_vec(&manifest).expect("manifest");
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "manifest.json", Cursor::new(content))
            .expect("append");
        builder.finish().expect("finish");
    }

    fn image_tar(path: &std::path::Path, tag: &str) {
        image_tar_manifest(path, serde_json::json!([{"RepoTags": [tag]}]));
    }

    #[test]
    fn selected_image_tag_must_exist_in_archive() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("app.tar");
        image_tar(&path, "repo/app:1");
        let input = DeploymentImageInput {
            service_name: "app".into(),
            file_path: path.to_string_lossy().into_owned(),
            image_tag: "repo/app:1".into(),
        };
        let inspected = inspect_deployment_images(std::slice::from_ref(&input)).expect("inspect");
        assert_eq!(inspected.image_files[0].image_tag, "repo/app:1");
        let mut wrong = input;
        wrong.image_tag = "repo/app:2".into();
        assert!(inspect_deployment_images(&[wrong]).is_err());
    }

    #[test]
    fn single_image_accepts_multiple_or_manual_tags_but_multi_image_tar_is_rejected() {
        let temp = tempfile::tempdir().expect("temp");
        let single = temp.path().join("single.tar");
        image_tar_manifest(&single, serde_json::json!([{"RepoTags": null}]));
        let inspected = inspect_deployment_images(&[DeploymentImageInput {
            service_name: "app".into(),
            file_path: single.to_string_lossy().into_owned(),
            image_tag: "registry.example/app:1".into(),
        }])
        .expect("manual tag");
        assert_eq!(inspected.image_files[0].image_tag, "registry.example/app:1");

        let multiple_tags = temp.path().join("multiple-tags.tar");
        image_tar_manifest(
            &multiple_tags,
            serde_json::json!([{"RepoTags": ["registry.example/app:latest", "registry.example/app:1"]}]),
        );
        let inspected = inspect_image_archive_with_limits(&multiple_tags, 1024 * 1024, 10, 1024)
            .expect("one image may expose multiple tags");
        assert_eq!(
            inspected.repo_tags,
            vec!["registry.example/app:1", "registry.example/app:latest"]
        );

        for (name, manifest) in [
            (
                "tagged",
                serde_json::json!([
                    {"RepoTags": ["registry.example/app:1"]},
                    {"RepoTags": ["registry.example/worker:1"]}
                ]),
            ),
            (
                "mixed",
                serde_json::json!([
                    {"RepoTags": ["registry.example/app:1"]},
                    {"RepoTags": null}
                ]),
            ),
            (
                "untagged",
                serde_json::json!([{"RepoTags": null}, {"RepoTags": ["<none>:<none>"]}]),
            ),
        ] {
            let path = temp.path().join(format!("multi-{name}.tar"));
            image_tar_manifest(&path, manifest);
            let error = inspect_image_archive_with_limits(&path, 1024 * 1024, 10, 1024)
                .expect_err("multi-image tar must be rejected");
            assert_eq!(
                error.to_string(),
                "配置无效：只能选择单镜像Tar，请重新导出镜像"
            );
        }
    }

    #[test]
    fn deployment_image_fingerprint_is_independent_of_selection_order() {
        let temp = tempfile::tempdir().expect("temp");
        let first = temp.path().join("first.tar");
        let second = temp.path().join("second.tar");
        image_tar(&first, "repo/first:1");
        image_tar(&second, "repo/second:2");
        let images = vec![
            DeploymentImageInput {
                service_name: "second".into(),
                file_path: second.to_string_lossy().into_owned(),
                image_tag: "repo/second:2".into(),
            },
            DeploymentImageInput {
                service_name: "first".into(),
                file_path: first.to_string_lossy().into_owned(),
                image_tag: "repo/first:1".into(),
            },
        ];
        let forward = inspect_deployment_images(&images).expect("forward");
        let reverse = inspect_deployment_images(&images.into_iter().rev().collect::<Vec<_>>())
            .expect("reverse");
        assert_eq!(forward.fingerprint, reverse.fingerprint);
        assert_eq!(forward.image_files, reverse.image_files);
    }

    #[test]
    fn image_archive_limits_are_enforced() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("app.tar");
        image_tar(&path, "repo/app:1");
        assert!(inspect_image_archive_with_limits(&path, 1, 10, 1024).is_err());
    }
}
