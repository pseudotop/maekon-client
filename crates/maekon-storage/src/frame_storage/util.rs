use crate::error::StorageError;
use std::path::Path;
use tokio::fs;
#[cfg(windows)]
use tracing::info;
use tracing::warn;

/// Read a frame directory, repairing legacy empty-DACL directories once on
/// Windows before surfacing `PermissionDenied` (#9276).
async fn read_dir_with_permission_repair(path: &Path) -> std::io::Result<fs::ReadDir> {
    match fs::read_dir(path).await {
        Ok(entries) => Ok(entries),
        Err(error) => {
            #[cfg(windows)]
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                crate::encryption::set_owner_only_directory_dacl(path)
                    .map_err(|repair_error| std::io::Error::other(repair_error.to_string()))?;
                info!(path = %path.display(), "frame directory DACL repaired before read");
                return fs::read_dir(path).await;
            }

            Err(error)
        }
    }
}

/// Delete one frame directory and return its direct frame count.
///
/// A legacy empty DACL is repaired exactly once on Windows before the delete is
/// retried. Persistent failure remains an error so callers can surface a
/// degraded retention/privacy state instead of reporting silent success.
pub(super) async fn remove_frame_dir_with_permission_repair(path: &Path) -> std::io::Result<usize> {
    let count = count_files_in_dir(path).await;

    match fs::remove_dir_all(path).await {
        Ok(()) => Ok(count),
        Err(error) => {
            #[cfg(windows)]
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                crate::encryption::set_owner_only_directory_dacl(path)
                    .map_err(|repair_error| std::io::Error::other(repair_error.to_string()))?;
                info!(path = %path.display(), "frame directory DACL repaired before deletion");
                let repaired_count = count_files_in_dir(path).await;
                fs::remove_dir_all(path).await?;
                return Ok(repaired_count);
            }

            Err(error)
        }
    }
}

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

    let mut entries = read_dir_with_permission_repair(path)
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

    let mut entries = read_dir_with_permission_repair(frames_dir)
        .await
        .map_err(|e| StorageError::Internal(format!("Failed to read frames directory: {e}")))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| StorageError::Internal(format!("Failed to read entry: {e}")))?
    {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.len() != 10 || name.chars().nth(4) != Some('-') {
            continue;
        }

        match entry.file_type().await {
            Ok(file_type) if file_type.is_dir() => dirs.push(name.to_string()),
            Ok(_) => {}
            Err(error) => {
                #[cfg(windows)]
                if error.kind() == std::io::ErrorKind::PermissionDenied {
                    crate::encryption::set_owner_only_directory_dacl(&path).map_err(
                        |repair_error| {
                            StorageError::Internal(format!(
                                "Failed to repair frame directory DACL: {repair_error}"
                            ))
                        },
                    )?;
                    let metadata = fs::metadata(&path).await.map_err(|metadata_error| {
                        StorageError::Internal(format!(
                            "Failed to read repaired frame directory metadata: {metadata_error}"
                        ))
                    })?;
                    if metadata.is_dir() {
                        dirs.push(name.to_string());
                    }
                    continue;
                }

                return Err(StorageError::Internal(format!(
                    "Failed to read frame entry type: {error}"
                )));
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
                match remove_frame_dir_with_permission_repair(&dir_path).await {
                    Ok(count) => Some(count),
                    Err(e) => {
                        warn!(
                            path = %dir_path.display(),
                            error = %e,
                            "frame folder delete warning"
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
