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
    "android-build/aapt.exe",
    "android-build/lib/apksigner.jar",
    "java/bin/java.exe",
];
#[cfg(not(windows))]
const REQUIRED: &[&str] = &["android/adb", "android-build/aapt", "android-build/lib/apksigner.jar", "java/bin/java"];

pub fn executable_name(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.into() }
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
    let parent = executable.parent().ok_or_else(|| AppError::NotFound("工作台程序目录不存在".into()))?;
    if parent.file_name().is_some_and(|name| name == "MacOS") && parent.parent().is_some_and(|p| p.file_name().is_some_and(|n| n == "Contents")) {
        return Ok(parent.parent().unwrap().join("Resources/tools"));
    }
    Ok(parent.join("tools"))
}

pub fn current() -> AppResult<Option<PathBuf>> {
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
    let bytes = std::fs::read(root.join("android-manifest.json")).map_err(|_| fail())?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| fail())?;
    if manifest.format_version != 1 || manifest.files.is_empty() {
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
    Ok(())
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
        assert_eq!(bundle_root(Path::new("portable/app.exe")).unwrap(), PathBuf::from("portable/tools"));
        assert_eq!(bundle_root(Path::new("Workbench.app/Contents/MacOS/app")).unwrap(), PathBuf::from("Workbench.app/Contents/Resources/tools"));
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
            assert!(validate(root).is_err(), "工具清单缺少 {field} 时不能继续运行");
        }
        let mut manifest = original;
        manifest.as_object_mut().unwrap().remove("versions");
        save(root, &manifest);
        assert!(validate(root).is_err());
    }
}
