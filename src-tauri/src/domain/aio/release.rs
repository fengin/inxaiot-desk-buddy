use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::core::error::{AppError, AppResult};

const REQUIRED_SERVICES: &[&str] = &["emqx", "device-edge", "rule-engine", "device-edge-web"];
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_IMAGE_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_RELEASE_VERSION_BYTES: usize = 128;
const MAX_RELEASE_FILE_COUNT: usize = 10_000;
const MAX_RELEASE_TOTAL_BYTES: u64 = 50 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseManifest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub version: String,
    pub compose_file: String,
    pub images: Vec<ReleaseImage>,
    pub templates: ReleaseTemplates,
    #[serde(default)]
    pub runtime: ReleaseRuntime,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseImage {
    pub service: String,
    pub image: String,
    #[serde(default)]
    pub archive: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseTemplates {
    pub env: String,
    pub host_info: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseRuntime {
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub docker: String,
    #[serde(default)]
    pub compose: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageArchiveInfo {
    pub path: String,
    pub size: u64,
    pub repo_tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseValidation {
    pub valid: bool,
    pub package_dir: String,
    pub manifest: Option<ReleaseManifest>,
    pub fingerprint: Option<String>,
    pub images: Vec<ImageArchiveInfo>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn inspect_release_directory(package_dir: &Path) -> AppResult<ReleaseValidation> {
    let root = canonical_directory(package_dir)?;
    let manifest_path = root.join("manifest.json");
    let mut errors = Vec::new();
    let manifest = match read_manifest(&manifest_path) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok(ReleaseValidation {
                valid: false,
                package_dir: root.to_string_lossy().into_owned(),
                manifest: None,
                fingerprint: None,
                images: Vec::new(),
                errors: vec![error.to_string()],
                warnings: Vec::new(),
            });
        }
    };
    validate_manifest_fields(&manifest, &mut errors);
    let compose = resolve_release_file(&root, &manifest.compose_file, "Compose", &mut errors);
    let env = resolve_release_file(&root, &manifest.templates.env, "环境变量模板", &mut errors);
    let _host_info = resolve_release_file(
        &root,
        &manifest.templates.host_info,
        "host-info模板",
        &mut errors,
    );
    if let (Some(compose), Some(env)) = (&compose, &env) {
        validate_compose_variables(compose, env, &mut errors)?;
    }
    let mut images = Vec::new();
    for image in &manifest.images {
        if image.archive.trim().is_empty() {
            errors.push(format!("服务 {} 未声明镜像归档", image.service));
            continue;
        }
        let Some(path) = resolve_release_file(&root, &image.archive, "镜像归档", &mut errors)
        else {
            continue;
        };
        match inspect_image_archive(&path) {
            Ok(info) => {
                if !info.repo_tags.iter().any(|tag| tag == &image.image) {
                    errors.push(format!(
                        "服务 {} 声明镜像 {} 与归档 RepoTags 不一致",
                        image.service, image.image
                    ));
                }
                images.push(info);
            }
            Err(error) => errors.push(format!("检查服务 {} 镜像失败：{error}", image.service)),
        }
    }
    let fingerprint = if errors.is_empty() {
        Some(fingerprint_directory(&root)?)
    } else {
        None
    };
    Ok(ReleaseValidation {
        valid: errors.is_empty(),
        package_dir: root.to_string_lossy().into_owned(),
        manifest: Some(manifest),
        fingerprint,
        images,
        errors,
        warnings: Vec::new(),
    })
}

pub fn inspect_image_archive(path: &Path) -> AppResult<ImageArchiveInfo> {
    let canonical = path
        .canonicalize()
        .map_err(|error| AppError::io("读取Docker镜像归档", &error))?;
    let metadata =
        std::fs::metadata(&canonical).map_err(|error| AppError::io("读取镜像属性", &error))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(AppError::InvalidConfig(
            "Docker镜像归档为空或不是文件".into(),
        ));
    }
    let file = std::fs::File::open(&canonical)
        .map_err(|error| AppError::io("打开Docker镜像归档", &error))?;
    let mut archive = tar::Archive::new(file);
    let entries = archive
        .entries()
        .map_err(|error| AppError::io("读取Docker镜像tar目录", &error))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| AppError::io("读取Docker镜像tar项", &error))?;
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
        let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| AppError::io("读取Docker镜像manifest.json", &error))?;
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct DockerManifestEntry {
            #[serde(default)]
            repo_tags: Vec<String>,
        }
        let records = serde_json::from_slice::<Vec<DockerManifestEntry>>(&bytes)
            .map_err(|_| AppError::InvalidConfig("Docker镜像manifest.json格式无效".into()))?;
        let repo_tags = records
            .into_iter()
            .flat_map(|record| record.repo_tags)
            .map(|tag| tag.trim().to_string())
            .filter(|tag| !tag.is_empty() && tag != "<none>:<none>")
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if repo_tags.is_empty() {
            return Err(AppError::InvalidConfig(
                "Docker镜像归档没有有效RepoTag".into(),
            ));
        }
        return Ok(ImageArchiveInfo {
            path: canonical.to_string_lossy().into_owned(),
            size: metadata.len(),
            repo_tags,
        });
    }
    Err(AppError::InvalidConfig(
        "Docker镜像归档缺少manifest.json".into(),
    ))
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
            "Release版本只能由字母、数字、点、下划线和连字符组成，且必须以字母或数字开头".into(),
        ));
    }
    Ok(())
}

fn read_manifest(path: &Path) -> AppResult<ReleaseManifest> {
    let metadata =
        std::fs::metadata(path).map_err(|error| AppError::io("读取Release manifest", &error))?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(AppError::InvalidConfig(
            "Release manifest.json超过1MiB".into(),
        ));
    }
    let bytes =
        std::fs::read(path).map_err(|error| AppError::io("读取Release manifest", &error))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::InvalidConfig("Release manifest.json格式无效".into()))
}

fn validate_manifest_fields(manifest: &ReleaseManifest, errors: &mut Vec<String>) {
    if manifest.schema_version != 1 {
        errors.push("manifest.schemaVersion当前只支持1".into());
    }
    if let Err(error) = validate_release_version(&manifest.version) {
        errors.push(error.to_string());
    }
    if manifest.compose_file.trim().is_empty() {
        errors.push("manifest.composeFile不能为空".into());
    }
    if manifest.runtime.os.trim().is_empty()
        || manifest.runtime.arch.trim().is_empty()
        || manifest.runtime.docker.trim().is_empty()
        || manifest.runtime.compose.trim().is_empty()
    {
        errors.push("manifest.runtime必须完整声明os、arch、docker和compose约束".into());
    }
    let mut services = BTreeSet::new();
    for image in &manifest.images {
        if image.service.trim().is_empty() || image.image.trim().is_empty() {
            errors.push("镜像service和image不能为空".into());
        } else if !services.insert(image.service.trim().to_string()) {
            errors.push(format!("服务名重复：{}", image.service));
        }
    }
    for service in REQUIRED_SERVICES {
        if !services.contains(*service) {
            errors.push(format!("缺少必需服务镜像：{service}"));
        }
    }
}

fn canonical_directory(path: &Path) -> AppResult<PathBuf> {
    let canonical = path
        .canonicalize()
        .map_err(|error| AppError::io("读取Release目录", &error))?;
    if !canonical.is_dir() {
        return Err(AppError::InvalidConfig("Release路径不是目录".into()));
    }
    Ok(canonical)
}

fn resolve_release_file(
    root: &Path,
    relative: &str,
    label: &str,
    errors: &mut Vec<String>,
) -> Option<PathBuf> {
    let relative_path = Path::new(relative.trim());
    if relative.trim().is_empty()
        || relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        errors.push(format!("{label}路径不安全：{relative}"));
        return None;
    }
    let candidate = root.join(relative_path);
    let canonical = match candidate.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            errors.push(format!("{label}不存在：{relative}"));
            return None;
        }
    };
    if !canonical.starts_with(root) || !canonical.is_file() {
        errors.push(format!("{label}逃逸Release目录或不是文件：{relative}"));
        return None;
    }
    Some(canonical)
}

fn validate_compose_variables(
    compose_path: &Path,
    env_path: &Path,
    errors: &mut Vec<String>,
) -> AppResult<()> {
    let compose = std::fs::read_to_string(compose_path)
        .map_err(|error| AppError::io("读取Compose文件", &error))?;
    let env = std::fs::read_to_string(env_path)
        .map_err(|error| AppError::io("读取环境变量模板", &error))?;
    let env_pattern = Regex::new(r"(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)=").expect("static env regex");
    let compose_pattern = Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?:(:-|:\?|-|\?)[^}]*)?\}")
        .expect("static compose regex");
    let defined = env_pattern
        .captures_iter(&env)
        .map(|capture| capture[1].to_string())
        .collect::<BTreeSet<_>>();
    let mut missing = BTreeSet::new();
    for capture in compose_pattern.captures_iter(&compose) {
        let modifier = capture.get(2).map(|value| value.as_str()).unwrap_or("");
        if !matches!(modifier, ":-" | "-") && !defined.contains(&capture[1]) {
            missing.insert(capture[1].to_string());
        }
    }
    for name in missing {
        errors.push(format!("环境变量模板未定义Compose变量 {name}"));
    }
    Ok(())
}

fn fingerprint_directory(root: &Path) -> AppResult<String> {
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|_| AppError::InvalidConfig("遍历Release目录失败".into()))?;
        if entry.file_type().is_symlink() {
            return Err(AppError::InvalidConfig(
                "Release目录不允许包含符号链接".into(),
            ));
        }
        if !entry.file_type().is_file() {
            continue;
        }
        files.push(entry.path().to_path_buf());
        if files.len() > MAX_RELEASE_FILE_COUNT {
            return Err(AppError::InvalidConfig(
                "Release文件数量超过10000个安全上限".into(),
            ));
        }
        total_bytes = total_bytes.saturating_add(
            entry
                .metadata()
                .map_err(|_| AppError::Io {
                    operation: "读取Release文件属性",
                })?
                .len(),
        );
        if total_bytes > MAX_RELEASE_TOTAL_BYTES {
            return Err(AppError::InvalidConfig(
                "Release总大小超过50GiB安全上限".into(),
            ));
        }
    }
    files.sort();
    let mut digest = Sha256::new();
    for path in files {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| AppError::InvalidConfig("Release文件路径异常".into()))?;
        digest.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        digest.update([0]);
        let mut file =
            std::fs::File::open(&path).map_err(|error| AppError::io("读取Release文件", &error))?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|error| AppError::io("计算Release指纹", &error))?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
    }
    Ok(hex::encode(digest.finalize()))
}

fn default_schema_version() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::Path;

    use super::{
        ReleaseImage, ReleaseManifest, ReleaseRuntime, ReleaseTemplates, inspect_image_archive,
        inspect_release_directory,
    };

    fn image_tar(path: &Path, repo_tag: &str) {
        let file = std::fs::File::create(path).expect("image tar");
        let mut builder = tar::Builder::new(file);
        let content = serde_json::to_vec(&serde_json::json!([
            { "Config": "config.json", "RepoTags": [repo_tag], "Layers": [] }
        ]))
        .expect("manifest");
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "manifest.json", Cursor::new(content))
            .expect("append manifest");
        builder.finish().expect("finish tar");
    }

    #[test]
    fn validates_complete_release_and_image_repo_tags() {
        let temp = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(temp.path().join("templates")).expect("templates");
        std::fs::create_dir_all(temp.path().join("images")).expect("images");
        std::fs::write(
            temp.path().join("docker-compose.yml"),
            "services:\n  app:\n    image: ${APP_IMAGE}\n",
        )
        .expect("compose");
        std::fs::write(
            temp.path().join("templates/env.template"),
            "APP_IMAGE={{images.device-edge}}\n",
        )
        .expect("env");
        std::fs::write(
            temp.path().join("templates/host-info.json.template"),
            "{}\n",
        )
        .expect("host info");
        let images = [
            ("emqx", "emqx:1"),
            ("device-edge", "device-edge:1"),
            ("rule-engine", "rule-engine:1"),
            ("device-edge-web", "device-edge-web:1"),
        ]
        .into_iter()
        .map(|(service, image)| {
            let archive = format!("images/{service}.tar");
            image_tar(&temp.path().join(&archive), image);
            ReleaseImage {
                service: service.into(),
                image: image.into(),
                archive,
            }
        })
        .collect::<Vec<_>>();
        let manifest = ReleaseManifest {
            schema_version: 1,
            version: "2026.08.28".into(),
            compose_file: "docker-compose.yml".into(),
            images,
            templates: ReleaseTemplates {
                env: "templates/env.template".into(),
                host_info: "templates/host-info.json.template".into(),
            },
            runtime: ReleaseRuntime {
                os: "linux".into(),
                arch: "x86_64".into(),
                docker: ">=20.10".into(),
                compose: ">=2.0".into(),
            },
        };
        std::fs::write(
            temp.path().join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).expect("json"),
        )
        .expect("manifest");
        let result = inspect_release_directory(temp.path()).expect("inspect");
        assert!(result.valid, "{:?}", result.errors);
        assert_eq!(result.images.len(), 4);
        assert!(result.fingerprint.is_some());
        let image =
            inspect_image_archive(&temp.path().join("images/device-edge.tar")).expect("image");
        assert_eq!(image.repo_tags, vec!["device-edge:1"]);
    }

    #[test]
    fn rejects_path_escape_missing_service_and_repo_tag_mismatch() {
        let temp = tempfile::tempdir().expect("temp");
        std::fs::write(
            temp.path().join("manifest.json"),
            r#"{
              "version":"bad",
              "composeFile":"../compose.yml",
              "images":[{"service":"emqx","image":"emqx:2","archive":"missing.tar"}],
              "templates":{"env":"missing.env","hostInfo":"missing.json"}
            }"#,
        )
        .expect("manifest");
        let result = inspect_release_directory(temp.path()).expect("inspect");
        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.contains("路径不安全"))
        );
        assert!(
            result
                .errors
                .iter()
                .any(|error| error.contains("device-edge"))
        );
    }
}
