use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::application::ports::file_processing::{
    FileProcessingProgress, FileProcessingProgressSink, FileProcessingService, FileProcessingStage,
    LocalFileDescriptor, OperationWorkspace,
};
use crate::core::error::{AppError, AppResult};

const BUFFER_SIZE: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct LocalFileProcessor {
    workspace_root: PathBuf,
}

impl LocalFileProcessor {
    pub async fn open(workspace_root: impl AsRef<Path>) -> AppResult<Self> {
        let workspace_root = workspace_root.as_ref();
        if workspace_root.as_os_str().is_empty() {
            return Err(AppError::InvalidConfig("任务工作目录不能为空".into()));
        }
        tokio::fs::create_dir_all(workspace_root)
            .await
            .map_err(|error| AppError::io("创建任务工作根目录", &error))?;
        let workspace_root = tokio::fs::canonicalize(workspace_root)
            .await
            .map_err(|error| AppError::io("规范化任务工作根目录", &error))?;
        Ok(Self { workspace_root })
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    async fn validate_selected_file(
        &self,
        selected_root: &Path,
        path: &Path,
    ) -> AppResult<PathBuf> {
        let root = tokio::fs::canonicalize(selected_root)
            .await
            .map_err(|error| AppError::io("规范化已选择目录", &error))?;
        let symlink_metadata = tokio::fs::symlink_metadata(path)
            .await
            .map_err(|error| AppError::io("读取已选择文件信息", &error))?;
        if symlink_metadata.file_type().is_symlink() {
            return Err(AppError::InvalidConfig("不允许选择符号链接文件".into()));
        }
        let path = tokio::fs::canonicalize(path)
            .await
            .map_err(|error| AppError::io("规范化已选择文件", &error))?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err(AppError::InvalidConfig(
                "文件必须位于用户已选择的目录内".into(),
            ));
        }
        Ok(path)
    }

    fn validate_workspace(&self, workspace: &OperationWorkspace) -> AppResult<()> {
        if workspace.root == self.workspace_root
            || !workspace.root.starts_with(&self.workspace_root)
        {
            return Err(AppError::InvalidConfig("任务工作目录越界".into()));
        }
        Ok(())
    }
}

impl FileProcessingService for LocalFileProcessor {
    async fn inspect_selected_file(
        &self,
        selected_root: &Path,
        path: &Path,
        cancellation: &CancellationToken,
        progress: &dyn FileProcessingProgressSink,
    ) -> AppResult<LocalFileDescriptor> {
        let path = self.validate_selected_file(selected_root, path).await?;
        inspect_file(&path, FileProcessingStage::Hashing, cancellation, progress).await
    }

    async fn create_workspace(
        &self,
        local_project_id: &str,
        local_task_id: &str,
    ) -> AppResult<OperationWorkspace> {
        validate_segment(local_project_id)?;
        validate_segment(local_task_id)?;
        let root = self
            .workspace_root
            .join(local_project_id)
            .join("tasks")
            .join(local_task_id);
        if root.exists() {
            return Err(AppError::Conflict(format!(
                "任务工作目录已存在：{local_task_id}"
            )));
        }
        let artifacts_dir = root.join("artifacts");
        let temporary_dir = root.join("tmp");
        tokio::fs::create_dir_all(&artifacts_dir)
            .await
            .map_err(|error| AppError::io("创建任务制品目录", &error))?;
        tokio::fs::create_dir_all(&temporary_dir)
            .await
            .map_err(|error| AppError::io("创建任务临时目录", &error))?;
        Ok(OperationWorkspace {
            log_path: root.join("events.jsonl"),
            root,
            artifacts_dir,
            temporary_dir,
        })
    }

    async fn copy_to_workspace(
        &self,
        source: &LocalFileDescriptor,
        workspace: &OperationWorkspace,
        target_name: &str,
        cancellation: &CancellationToken,
        progress: &dyn FileProcessingProgressSink,
    ) -> AppResult<LocalFileDescriptor> {
        self.validate_workspace(workspace)?;
        validate_segment(target_name)?;
        if !source.path.is_file() {
            return Err(AppError::InvalidConfig("待复制源文件不存在".into()));
        }
        let destination = workspace.artifacts_dir.join(target_name);
        let temporary = workspace.temporary_dir.join(format!("{target_name}.part"));
        let copy_result = copy_file(
            &source.path,
            &temporary,
            source.size,
            cancellation,
            progress,
        )
        .await;
        if let Err(error) = copy_result {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error);
        }
        let copied = inspect_file(
            &temporary,
            FileProcessingStage::Verifying,
            cancellation,
            progress,
        )
        .await?;
        if copied.size != source.size || !copied.sha256.eq_ignore_ascii_case(&source.sha256) {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(AppError::Integrity {
                operation: "任务工作区文件复制校验",
            });
        }
        tokio::fs::rename(&temporary, &destination)
            .await
            .map_err(|error| AppError::io("发布任务工作区文件", &error))?;
        Ok(LocalFileDescriptor {
            path: destination,
            size: copied.size,
            sha256: copied.sha256,
        })
    }

    async fn cleanup_workspace(&self, workspace: &OperationWorkspace) -> AppResult<()> {
        self.validate_workspace(workspace)?;
        if !workspace.root.exists() {
            return Ok(());
        }
        let canonical = tokio::fs::canonicalize(&workspace.root)
            .await
            .map_err(|error| AppError::io("规范化待清理任务目录", &error))?;
        if canonical == self.workspace_root || !canonical.starts_with(&self.workspace_root) {
            return Err(AppError::InvalidConfig("拒绝清理越界任务目录".into()));
        }
        tokio::fs::remove_dir_all(&canonical)
            .await
            .map_err(|error| AppError::io("清理任务工作目录", &error))
    }
}

async fn inspect_file(
    path: &Path,
    stage: FileProcessingStage,
    cancellation: &CancellationToken,
    progress: &dyn FileProcessingProgressSink,
) -> AppResult<LocalFileDescriptor> {
    let total = tokio::fs::metadata(path)
        .await
        .map_err(|error| AppError::io("读取文件信息", &error))?
        .len();
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| AppError::io("打开待处理文件", &error))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    let mut processed = 0_u64;
    loop {
        if cancellation.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| AppError::io("读取待处理文件", &error))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        processed += read as u64;
        progress.emit(FileProcessingProgress {
            stage,
            processed,
            total,
        })?;
    }
    Ok(LocalFileDescriptor {
        path: path.to_path_buf(),
        size: total,
        sha256: hex::encode(hash.finalize()),
    })
}

async fn copy_file(
    source: &Path,
    destination: &Path,
    total: u64,
    cancellation: &CancellationToken,
    progress: &dyn FileProcessingProgressSink,
) -> AppResult<()> {
    let mut source = tokio::fs::File::open(source)
        .await
        .map_err(|error| AppError::io("打开待复制文件", &error))?;
    let mut destination = tokio::fs::File::create(destination)
        .await
        .map_err(|error| AppError::io("创建复制临时文件", &error))?;
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    let mut processed = 0_u64;
    loop {
        if cancellation.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let read = source
            .read(&mut buffer)
            .await
            .map_err(|error| AppError::io("读取待复制文件", &error))?;
        if read == 0 {
            break;
        }
        destination
            .write_all(&buffer[..read])
            .await
            .map_err(|error| AppError::io("写入复制临时文件", &error))?;
        processed += read as u64;
        progress.emit(FileProcessingProgress {
            stage: FileProcessingStage::Copying,
            processed,
            total,
        })?;
    }
    destination
        .flush()
        .await
        .map_err(|error| AppError::io("刷新复制临时文件", &error))
}

fn validate_segment(value: &str) -> AppResult<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.contains('\0')
    {
        return Err(AppError::InvalidConfig("任务路径段无效".into()));
    }
    Ok(())
}
