use crate::error::StorageError;
use std::path::Path;
use tokio::fs;
use tracing::warn;

pub(super) async fn count_files_in_dir(path: &Path) -> usize {
    let mut count = 0;
    if let Ok(mut entries) = fs::read_dir(path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.path().is_file() {
                count += 1;
            }
        }
    }
    count
}

pub(super) async fn calculate_dir_size(path: &Path) -> Result<u64, StorageError> {
    let mut total = 0u64;

    let mut entries = fs::read_dir(path)
        .await
        .map_err(|e| StorageError::Internal(format!("Failed to read directory: {e}")))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| StorageError::Internal(format!("Failed to read entry: {e}")))?
    {
        let path = entry.path();
        let metadata = fs::metadata(&path)
            .await
            .map_err(|e| StorageError::Internal(format!("Failed to read metadata: {e}")))?;

        if metadata.is_file() {
            total += metadata.len();
        } else if metadata.is_dir() {
            total += Box::pin(calculate_dir_size(&path)).await?;
        }
    }

    Ok(total)
}

pub(super) async fn list_date_dirs(frames_dir: &Path) -> Result<Vec<String>, StorageError> {
    let mut dirs = Vec::with_capacity(365);

    if !frames_dir.exists() {
        return Ok(dirs);
    }

    let mut entries = fs::read_dir(frames_dir)
        .await
        .map_err(|e| StorageError::Internal(format!("Failed to read frames directory: {e}")))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| StorageError::Internal(format!("Failed to read entry: {e}")))?
    {
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.len() == 10 && name.chars().nth(4) == Some('-') {
                    dirs.push(name.to_string());
                }
            }
        }
    }

    Ok(dirs)
}

/// Delete a batch of date-named directories, logging warnings on failure.
/// Returns the total number of frame files deleted.
pub(super) async fn delete_date_dirs_chunked(
    frames_dir: &Path,
    dirs: &[String],
    chunk_size: usize,
) -> usize {
    let mut deleted = 0usize;
    for chunk in dirs.chunks(chunk_size) {
        let mut handles = Vec::with_capacity(chunk.len());
        for dir_name in chunk {
            let dir_path = frames_dir.join(dir_name);
            handles.push(tokio::spawn(async move {
                let count = count_files_in_dir(&dir_path).await;
                match fs::remove_dir_all(&dir_path).await {
                    Ok(()) => Some(count),
                    Err(e) => {
                        warn!(
                            "frame folder delete warning: {} — {}",
                            dir_path.display(),
                            e
                        );
                        None
                    }
                }
            }));
        }
        for handle in handles {
            if let Ok(Some(count)) = handle.await {
                deleted += count;
            }
        }
    }
    deleted
}
