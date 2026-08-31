use std::path::{Component, Path, PathBuf};

use walkdir::WalkDir;

use crate::core::error::{AppError, AppResult};

pub fn create_release_tar(source_dir: &Path, destination: &Path) -> AppResult<PathBuf> {
    let root = source_dir
        .canonicalize()
        .map_err(|error| AppError::io("读取Release归档源目录", &error))?;
    if !root.is_dir() {
        return Err(AppError::InvalidConfig("Release归档源不是目录".into()));
    }
    if destination.starts_with(&root) {
        return Err(AppError::InvalidConfig(
            "Release归档输出不能位于源目录内".into(),
        ));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| AppError::io("创建Release归档目录", &error))?;
    }
    let file = std::fs::File::create(destination)
        .map_err(|error| AppError::io("创建Release归档", &error))?;
    let mut builder = tar::Builder::new(file);
    for entry in WalkDir::new(&root).follow_links(false) {
        let entry = entry.map_err(|_| AppError::InvalidConfig("遍历Release目录失败".into()))?;
        if entry.path() == root {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(AppError::InvalidConfig(
                "Release目录不允许包含符号链接".into(),
            ));
        }
        let relative = entry
            .path()
            .strip_prefix(&root)
            .map_err(|_| AppError::InvalidConfig("Release归档路径异常".into()))?;
        if relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(AppError::InvalidConfig("Release归档路径不安全".into()));
        }
        if entry.file_type().is_dir() {
            builder
                .append_dir(relative, entry.path())
                .map_err(|error| AppError::io("归档Release目录", &error))?;
        } else if entry.file_type().is_file() {
            builder
                .append_path_with_name(entry.path(), relative)
                .map_err(|error| AppError::io("归档Release文件", &error))?;
        }
    }
    builder
        .finish()
        .map_err(|error| AppError::io("完成Release归档", &error))?;
    Ok(destination.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::create_release_tar;

    #[test]
    fn creates_tar_outside_source_without_absolute_entries() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("release");
        std::fs::create_dir_all(source.join("templates")).expect("dirs");
        std::fs::write(source.join("manifest.json"), "{}").expect("manifest");
        std::fs::write(source.join("templates/env.template"), "A=1").expect("env");
        let output = temp.path().join("out/release.tar");
        create_release_tar(&source, &output).expect("archive");
        let file = std::fs::File::open(output).expect("open");
        let mut archive = tar::Archive::new(file);
        let paths = archive
            .entries()
            .expect("entries")
            .map(|entry| entry.expect("entry").path().expect("path").into_owned())
            .collect::<Vec<_>>();
        assert!(paths.iter().all(|path| !path.is_absolute()));
        assert!(paths.iter().any(|path| path.ends_with("manifest.json")));
    }
}
