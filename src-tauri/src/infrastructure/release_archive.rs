use std::io::Read;
use std::path::{Component, Path, PathBuf};

use tokio_util::sync::CancellationToken;

use walkdir::WalkDir;

use crate::core::error::{AppError, AppResult};

const MAX_RELEASE_ARCHIVE_FILE_COUNT: usize = 10_000;
const MAX_RELEASE_ARCHIVE_TOTAL_BYTES: u64 = 50 * 1024 * 1024 * 1024;

pub fn create_generated_release_tar(source_dir: &Path, destination: &Path) -> AppResult<PathBuf> {
    create_generated_release_tar_observed(
        source_dir,
        destination,
        &CancellationToken::new(),
        |_, _| Ok(()),
    )
}

pub fn create_generated_release_tar_observed(
    source_dir: &Path,
    destination: &Path,
    cancellation: &CancellationToken,
    mut on_progress: impl FnMut(u64, u64) -> AppResult<()>,
) -> AppResult<PathBuf> {
    let root = source_dir
        .canonicalize()
        .map_err(|error| AppError::io("读取内部发布包目录", &error))?;
    if !root.is_dir() {
        return Err(AppError::InvalidConfig("内部发布包源不是目录".into()));
    }
    if destination.starts_with(&root) {
        return Err(AppError::InvalidConfig(
            "内部发布包输出不能位于源目录内".into(),
        ));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| AppError::io("创建内部发布包目录", &error))?;
    }
    let mut entries = Vec::new();
    let mut file_count = 0_usize;
    let mut total_bytes = 0_u64;
    for entry in WalkDir::new(&root).follow_links(false) {
        if cancellation.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let entry = entry.map_err(|_| AppError::InvalidConfig("遍历内部发布包失败".into()))?;
        if entry.path() == root {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(AppError::InvalidConfig(
                "内部发布包不允许包含符号链接".into(),
            ));
        }
        let relative = entry
            .path()
            .strip_prefix(&root)
            .map_err(|_| AppError::InvalidConfig("内部发布包路径异常".into()))?;
        if relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(AppError::InvalidConfig("内部发布包路径不安全".into()));
        }
        if entry.file_type().is_dir() {
            entries.push((entry.path().to_path_buf(), relative.to_path_buf(), None));
        } else if entry.file_type().is_file() {
            file_count = file_count.saturating_add(1);
            if file_count > MAX_RELEASE_ARCHIVE_FILE_COUNT {
                return Err(AppError::InvalidConfig(
                    "内部发布包文件数量超过10000个安全上限".into(),
                ));
            }
            let size = entry
                .metadata()
                .map_err(|_| AppError::Io {
                    operation: "读取内部发布包文件属性",
                })?
                .len();
            total_bytes = total_bytes.saturating_add(size);
            if total_bytes > MAX_RELEASE_ARCHIVE_TOTAL_BYTES {
                return Err(AppError::InvalidConfig(
                    "内部发布包总大小超过50GiB安全上限".into(),
                ));
            }
            entries.push((
                entry.path().to_path_buf(),
                relative.to_path_buf(),
                Some(size),
            ));
        }
    }
    let file = std::fs::File::create(destination)
        .map_err(|error| AppError::io("创建内部发布包", &error))?;
    let mut builder = tar::Builder::new(file);
    let mut processed = 0_u64;
    on_progress(processed, total_bytes.max(1))?;
    for (path, relative, size) in entries {
        if cancellation.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        if let Some(size) = size {
            let file = std::fs::File::open(&path)
                .map_err(|error| AppError::io("打开内部发布包文件", &error))?;
            let mut progress_error = None;
            let mut reader = ArchiveProgressReader {
                inner: file,
                processed: &mut processed,
                total: total_bytes.max(1),
                cancellation,
                on_progress: &mut on_progress,
                progress_error: &mut progress_error,
            };
            let mut header = tar::Header::new_gnu();
            header.set_size(size);
            header.set_mode(0o644);
            header.set_cksum();
            if let Err(error) = builder.append_data(&mut header, relative, &mut reader) {
                if let Some(error) = progress_error {
                    return Err(error);
                }
                if cancellation.is_cancelled() {
                    return Err(AppError::Cancelled);
                }
                return Err(AppError::io("归档内部发布包文件", &error));
            }
        } else {
            builder
                .append_dir(relative, path)
                .map_err(|error| AppError::io("归档内部发布包目录", &error))?;
        }
    }
    builder
        .finish()
        .map_err(|error| AppError::io("完成内部发布包归档", &error))?;
    on_progress(total_bytes.max(1), total_bytes.max(1))?;
    Ok(destination.to_path_buf())
}

struct ArchiveProgressReader<'a, R, F> {
    inner: R,
    processed: &'a mut u64,
    total: u64,
    cancellation: &'a CancellationToken,
    on_progress: &'a mut F,
    progress_error: &'a mut Option<AppError>,
}

impl<R, F> Read for ArchiveProgressReader<'_, R, F>
where
    R: Read,
    F: FnMut(u64, u64) -> AppResult<()>,
{
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancellation.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "deployment cancelled",
            ));
        }
        let count = self.inner.read(buffer)?;
        *self.processed = self
            .processed
            .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
        if let Err(error) = (self.on_progress)(*self.processed, self.total) {
            *self.progress_error = Some(error);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "deployment progress failed",
            ));
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::create_generated_release_tar;

    #[test]
    fn creates_tar_outside_source_without_absolute_entries() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("release");
        std::fs::create_dir_all(source.join("templates")).expect("dirs");
        std::fs::write(source.join("manifest.json"), "{}").expect("manifest");
        std::fs::write(source.join("templates/env.template"), "A=1").expect("env");
        let output = temp.path().join("out/release.tar");
        create_generated_release_tar(&source, &output).expect("archive");
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
