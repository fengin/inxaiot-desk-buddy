use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::formal::error::{FormalError, FormalResult};

pub struct DataDirectoryProcessLock {
    _file: File,
    path: PathBuf,
}

impl DataDirectoryProcessLock {
    pub fn acquire(path: impl AsRef<Path>) -> FormalResult<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                tracing::error!(
                    path = %path.display(),
                    error_kind = ?error.kind(),
                    "open data directory process lock failed"
                );
                FormalError::LocalIo("打开工作台数据目录进程锁")
            })?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(FormalError::Conflict(format!(
                    "工作台数据目录正在被另一个进程使用：{}；请切回已打开的工作台窗口",
                    path.parent().unwrap_or(&path).display()
                )));
            }
            Err(TryLockError::Error(error)) => {
                tracing::error!(
                    path = %path.display(),
                    error_kind = ?error.kind(),
                    "lock data directory failed"
                );
                return Err(FormalError::LocalIo("锁定工作台数据目录"));
            }
        }
        write_owner_metadata(&mut file, &path)?;
        Ok(Self { _file: file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn write_owner_metadata(file: &mut File, path: &Path) -> FormalResult<()> {
    file.set_len(0)
        .and_then(|_| file.seek(SeekFrom::Start(0)).map(|_| ()))
        .and_then(|_| writeln!(file, "pid={}", std::process::id()))
        .and_then(|_| file.sync_data())
        .map_err(|error| {
            tracing::error!(
                path = %path.display(),
                error_kind = ?error.kind(),
                "write data directory process lock metadata failed"
            );
            FormalError::LocalIo("写入工作台数据目录进程锁")
        })
}
