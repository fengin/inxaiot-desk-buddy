use crate::core::error::{AppError, AppResult};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::Read,
    path::{Component, Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format_version: u32,
    versions: ToolVersions,
    files: Vec<Entry>,
}
#[derive(Deserialize)]
struct ToolVersions {
    os: String,
    architecture: String,
}
#[derive(Deserialize)]
struct Entry {
    path: String,
    sha256: String,
}

#[cfg(windows)]
const REQUIRED: &[&str] = &[
    "android/adb.exe",
    "android/AdbWinApi.dll",
    "android/AdbWinUsbApi.dll",
    "android/NOTICE.txt",
    "android/source.properties",
];
#[cfg(not(windows))]
const REQUIRED: &[&str] = &[
    "android/adb",
    "android/NOTICE.txt",
    "android/source.properties",
];

pub const EMBEDDED_ADB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/embedded-adb.json.gz"));

pub fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.into()
    }
}

fn runtime_platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        platform => platform,
    }
}

fn runtime_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        architecture => architecture,
    }
}

pub fn bundle_root(executable: &Path) -> AppResult<PathBuf> {
    let parent = executable
        .parent()
        .ok_or_else(|| AppError::NotFound("工作台程序目录不存在".into()))?;
    if parent.file_name().is_some_and(|name| name == "MacOS")
        && parent
            .parent()
            .is_some_and(|p| p.file_name().is_some_and(|n| n == "Contents"))
    {
        return Ok(parent.parent().unwrap().join("Resources/tools"));
    }
    Ok(parent.join("tools"))
}

pub fn current() -> AppResult<Option<PathBuf>> {
    if !EMBEDDED_ADB.is_empty() {
        let base = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| AppError::InvalidConfig("无法取得当前用户的工具缓存目录".into()))?;
        return materialize_embedded(
            EMBEDDED_ADB,
            &PathBuf::from(base).join("com.inxaiot.desk-buddy/tool-cache"),
        )
        .map(Some);
    }
    let executable =
        std::env::current_exe().map_err(|error| AppError::io("读取工作台程序位置", &error))?;
    let root = bundle_root(&executable)?;
    if !root.join("android-manifest.json").exists()
        && !["android", "android-build", "java"]
            .iter()
            .any(|name| root.join(name).exists())
    {
        return Ok(None);
    }
    validate(&root)?;
    Ok(Some(root))
}

fn validate(root: &Path) -> AppResult<()> {
    let fail = || AppError::Integrity {
        operation: "随附 Android 工具缺失或校验失败，请重新复制完整工具目录",
    };
    let manifest_path = root.join("android-manifest.json");
    if std::fs::metadata(&manifest_path).map_err(|_| fail())?.len() > 1024 * 1024 {
        return Err(fail());
    }
    let bytes = std::fs::read(manifest_path).map_err(|_| fail())?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| fail())?;
    if ![1, 2].contains(&manifest.format_version) || manifest.files.is_empty() {
        return Err(fail());
    }
    // 清单标记的是配套工作台的目标平台；单个 Android 工具可以是 Universal 或通过 Rosetta 运行。
    if manifest.versions.os != runtime_platform()
        || manifest.versions.architecture != runtime_architecture()
    {
        return Err(AppError::Integrity {
            operation: "随附 Android 工具与当前工作台系统或架构不一致，请使用对应版本的完整发布包",
        });
    }
    let canonical = root.canonicalize().map_err(|_| fail())?;
    let mut paths = BTreeSet::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    for entry in manifest.files {
        let relative = Path::new(&entry.path);
        if relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
            || entry.sha256.len() != 64
            || !entry.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || !paths.insert(entry.path.clone())
        {
            return Err(fail());
        }
        let file = root.join(relative).canonicalize().map_err(|_| fail())?;
        if !file.starts_with(&canonical) {
            return Err(fail());
        }
        let mut input = std::fs::File::open(file).map_err(|_| fail())?;
        let mut hash = Sha256::new();
        loop {
            let count = input.read(&mut buffer).map_err(|_| fail())?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        if hex::encode(hash.finalize()) != entry.sha256.to_ascii_lowercase() {
            return Err(fail());
        }
    }
    if REQUIRED.iter().any(|name| !paths.contains(*name)) {
        return Err(fail());
    }
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|_| fail())?;
        if entry.path() == root {
            continue;
        }
        reject_link(entry.path())?;
        if entry.file_type().is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| fail())?
                .to_string_lossy()
                .replace('\\', "/");
            if relative != "android-manifest.json" && !paths.contains(&relative) {
                return Err(fail());
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddedBundle {
    format_version: u32,
    files: Vec<EmbeddedFile>,
}
#[derive(Deserialize)]
struct EmbeddedFile {
    path: String,
    data: String,
}

fn reject_link(path: &Path) -> AppResult<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        let linked = metadata.file_type().is_symlink();
        #[cfg(windows)]
        let linked = {
            use std::os::windows::fs::MetadataExt;
            linked || metadata.file_attributes() & 0x400 != 0
        };
        if linked {
            return Err(AppError::InvalidConfig(
                "ADB 工具缓存目录或文件不能是链接，请移除该链接后重试".into(),
            ));
        }
    }
    Ok(())
}

pub fn materialize_embedded(bytes: &[u8], base: &Path) -> AppResult<PathBuf> {
    use base64::Engine;
    use std::io::Write;
    let fail = || AppError::Integrity {
        operation: "内嵌 ADB 工具校验失败，请重新下载完整程序",
    };
    for ancestor in base.ancestors() {
        reject_link(ancestor)?;
    }
    std::fs::create_dir_all(base).map_err(|e| AppError::io("创建 ADB 工具缓存目录", &e))?;
    let id = hex::encode(Sha256::digest(bytes));
    let root = base.join(&id);
    let lock_path = base.join(format!("{id}.lock"));
    reject_link(&lock_path)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|e| AppError::io("打开 ADB 工具准备锁", &e))?;
    lock.lock()
        .map_err(|e| AppError::io("等待 ADB 工具准备", &e))?;
    reject_link(&root)?;
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut decoded)
        .map_err(|_| fail())?;
    if decoded.len() > 64 * 1024 * 1024 {
        return Err(fail());
    }
    let bundle: EmbeddedBundle = serde_json::from_slice(&decoded).map_err(|_| fail())?;
    if bundle.format_version != 1 || bundle.files.is_empty() || bundle.files.len() > 128 {
        return Err(fail());
    }
    let manifest = bundle
        .files
        .iter()
        .find(|file| file.path == "android-manifest.json")
        .ok_or_else(fail)?;
    let expected_manifest = base64::engine::general_purpose::STANDARD
        .decode(&manifest.data)
        .map_err(|_| fail())?;
    // 缓存中的清单也必须与EXE内原件一致，不能只信任可被同时改写的文件与清单。
    let manifest_path = root.join("android-manifest.json");
    let same_manifest = std::fs::metadata(&manifest_path)
        .is_ok_and(|meta| meta.len() == expected_manifest.len() as u64)
        && std::fs::read(&manifest_path).is_ok_and(|content| content == expected_manifest);
    if root.is_dir() && same_manifest && validate(&root).is_ok() {
        return Ok(root);
    }
    let staging = base.join(format!(".{id}-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&staging).map_err(|e| AppError::io("创建 ADB 准备目录", &e))?;
    let prepare = (|| {
        let mut names = BTreeSet::new();
        for file in bundle.files {
            let relative = Path::new(&file.path);
            if file.path.is_empty()
                || relative
                    .components()
                    .any(|p| !matches!(p, Component::Normal(_)))
                || !names.insert(file.path.clone())
            {
                return Err(fail());
            }
            let target = staging.join(relative);
            std::fs::create_dir_all(target.parent().unwrap())
                .map_err(|e| AppError::io("创建 ADB 文件目录", &e))?;
            let content = base64::engine::general_purpose::STANDARD
                .decode(file.data)
                .map_err(|_| fail())?;
            let mut output = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(target)
                .map_err(|e| AppError::io("准备 ADB 工具文件", &e))?;
            output
                .write_all(&content)
                .and_then(|_| output.sync_all())
                .map_err(|e| AppError::io("保存 ADB 工具文件", &e))?;
        }
        validate(&staging)?;
        if root.exists() {
            // 不覆盖运行中的 ADB 文件；损坏版本移开保留，下一次仅使用完整的新目录。
            let old = base.join(format!(".{id}-old-{}", uuid::Uuid::now_v7()));
            std::fs::rename(&root, old)
                .map_err(|e| AppError::io("更新损坏的 ADB 缓存，请关闭占用它的程序后重试", &e))?;
        }
        std::fs::rename(&staging, &root).map_err(|e| AppError::io("完成 ADB 工具准备", &e))?;
        Ok(root.clone())
    })();
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    prepare
}

/// 打包后独立进程验收：只运行 ADB version 和可选的本机 APK 解析，不连接设备。
pub async fn verify_distribution(apk: Option<&Path>) -> AppResult<serde_json::Value> {
    let root = current()?.ok_or_else(|| AppError::NotFound("程序缺少配套的 ADB 工具".into()))?;
    let output = super::device::run(
        &root.join("android").join(executable_name("adb")),
        &["version".into()],
        std::time::Duration::from_secs(15),
        tokio_util::sync::CancellationToken::new(),
    )
    .await?;
    if !output.success {
        return Err(AppError::InvalidConfig("配套 ADB 无法运行".into()));
    }
    let info = if let Some(path) = apk {
        Some(super::apk::inspect(path, tokio_util::sync::CancellationToken::new()).await?)
    } else {
        None
    };
    Ok(
        serde_json::json!({"successful":true,"embedded":!EMBEDDED_ADB.is_empty(),"toolRoot":root,"adbVersion":output.stdout.trim(),"apk":info}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(root: &Path) -> serde_json::Value {
        let entries: Vec<_> = REQUIRED.iter().map(|name| {
            let path = root.join(name); std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"tool-fixture").unwrap();
            serde_json::json!({"path":name,"sha256":hex::encode(Sha256::digest(b"tool-fixture"))})
        }).collect();
        serde_json::json!({
            "formatVersion":1,
            "versions":{"os":runtime_platform(),"architecture":runtime_architecture()},
            "files":entries
        })
    }
    fn save(root: &Path, value: &serde_json::Value) {
        std::fs::write(
            root.join("android-manifest.json"),
            serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn missing_corrupt_and_escaping_bundle_files_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let original = fixture(root);
        save(root, &original);
        assert!(validate(root).is_ok());
        std::fs::write(root.join(REQUIRED[0]), b"changed").unwrap();
        assert!(validate(root).is_err());
        fixture(root);
        std::fs::remove_file(root.join(REQUIRED.last().unwrap())).unwrap();
        assert!(validate(root).is_err());
        fixture(root);
        let mut escaping = original.clone();
        escaping["files"][0]["path"] = "../outside.exe".into();
        save(root, &escaping);
        assert!(validate(root).is_err());
        let mut incomplete = original;
        incomplete["files"].as_array_mut().unwrap().pop();
        save(root, &incomplete);
        assert!(validate(root).is_err());
    }

    #[test]
    fn resolves_windows_portable_and_macos_app_layouts() {
        assert_eq!(
            bundle_root(Path::new("portable/app.exe")).unwrap(),
            PathBuf::from("portable/tools")
        );
        assert_eq!(
            bundle_root(Path::new("Workbench.app/Contents/MacOS/app")).unwrap(),
            PathBuf::from("Workbench.app/Contents/Resources/tools")
        );
    }

    #[test]
    fn bundle_platform_and_architecture_must_match_the_application() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let original = fixture(root);
        for platform in ["win32", "darwin", "linux"] {
            for architecture in ["x64", "arm64"] {
                let mut manifest = original.clone();
                manifest["versions"]["os"] = platform.into();
                manifest["versions"]["architecture"] = architecture.into();
                save(root, &manifest);
                assert_eq!(
                    validate(root).is_ok(),
                    platform == runtime_platform() && architecture == runtime_architecture(),
                    "工具包 {platform}/{architecture} 与当前工作台的匹配判断错误"
                );
            }
        }
    }

    #[test]
    fn missing_platform_metadata_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let original = fixture(root);
        for field in ["os", "architecture"] {
            let mut manifest = original.clone();
            manifest["versions"].as_object_mut().unwrap().remove(field);
            save(root, &manifest);
            assert!(
                validate(root).is_err(),
                "工具清单缺少 {field} 时不能继续运行"
            );
        }
        let mut manifest = original;
        manifest.as_object_mut().unwrap().remove("versions");
        save(root, &manifest);
        assert!(validate(root).is_err());
    }

    fn embedded_fixture() -> Vec<u8> {
        use base64::Engine;
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        let value = fixture(temp.path());
        save(temp.path(), &value);
        let files = REQUIRED.iter().copied().chain(["android-manifest.json"]).map(|path| serde_json::json!({"path":path,"data":base64::engine::general_purpose::STANDARD.encode(std::fs::read(temp.path().join(path)).unwrap())})).collect::<Vec<_>>();
        let mut output = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        output
            .write_all(
                &serde_json::to_vec(&serde_json::json!({"formatVersion":1,"files":files})).unwrap(),
            )
            .unwrap();
        output.finish().unwrap()
    }
    #[test]
    fn embedded_tools_are_reused_repaired_and_published_once() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = embedded_fixture();
        let root = materialize_embedded(&bytes, directory.path()).unwrap();
        let first = std::fs::metadata(root.join(REQUIRED[0]))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            materialize_embedded(&bytes, directory.path()).unwrap(),
            root
        );
        assert_eq!(
            std::fs::metadata(root.join(REQUIRED[0]))
                .unwrap()
                .modified()
                .unwrap(),
            first
        );
        std::fs::write(root.join(REQUIRED[0]), b"bad").unwrap();
        assert_eq!(
            materialize_embedded(&bytes, directory.path()).unwrap(),
            root
        );
        assert!(validate(&root).is_ok());
        let mut forged: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("android-manifest.json")).unwrap())
                .unwrap();
        std::fs::write(root.join(REQUIRED[0]), b"replacement").unwrap();
        forged["files"][0]["sha256"] = hex::encode(Sha256::digest(b"replacement")).into();
        save(&root, &forged);
        materialize_embedded(&bytes, directory.path()).unwrap();
        assert_eq!(
            std::fs::read(root.join(REQUIRED[0])).unwrap(),
            b"tool-fixture"
        );
        std::fs::remove_dir_all(&root).unwrap();
        std::thread::scope(|scope| {
            let workers = (0..4)
                .map(|_| scope.spawn(|| materialize_embedded(&bytes, directory.path()).unwrap()))
                .collect::<Vec<_>>();
            for worker in workers {
                assert_eq!(worker.join().unwrap(), root);
            }
        });
        assert!(validate(&root).is_ok());
        assert!(materialize_embedded(b"invalid archive", directory.path()).is_err());
    }
}
