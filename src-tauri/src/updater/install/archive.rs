//! Archive extraction (tar.gz, zip) and binary discovery.

use std::path::{Component, Path, PathBuf};

use super::super::{UpdateError, Updater};

/// Hard cap on the *cumulative uncompressed* output of an archive extraction.
/// `download.rs` caps the COMPRESSED artifact (`MAX_UPDATE_BYTES`, 2 GiB), but
/// gzip/deflate routinely reach >1000:1 on repetitive data, so without this cap an
/// archive that passed the download check could still decompress to hundreds of
/// GiB and exhaust the disk. 4 GiB is far above any real maekon release payload.
const MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;

impl Updater {
    pub(crate) fn is_safe_archive_path(path: &Path) -> bool {
        path.components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
    }

    pub(crate) fn extract_tar_gz(&self, archive_path: &Path) -> Result<PathBuf, UpdateError> {
        use flate2::read::GzDecoder;
        use std::fs::File;

        let file = File::open(archive_path)?;
        let decoder = GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);

        let extract_dir = archive_path
            .parent()
            .unwrap_or(std::path::Path::new("/tmp"));
        let mut total_extracted: u64 = 0;
        for entry in archive.entries()? {
            let mut entry = entry?;
            let entry_path = entry.path()?;

            if !Self::is_safe_archive_path(&entry_path) {
                return Err(UpdateError::Install(format!(
                    "Unsafe tar entry path: {}",
                    entry_path.display()
                )));
            }

            let outpath = extract_dir.join(entry_path.as_ref());

            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let entry_type = entry.header().entry_type();
            if entry_type.is_dir() {
                std::fs::create_dir_all(&outpath)?;
                continue;
            }

            if !entry_type.is_file() {
                return Err(UpdateError::Install(format!(
                    "Unsupported tar entry type: {}",
                    entry_path.display()
                )));
            }

            // tar `unpack` reads exactly `header.size()` bytes from the (decompressing)
            // gz stream, so the declared size is authoritative — tracking it bounds the
            // total decompressed output and aborts a gzip bomb before it fills the disk.
            total_extracted = total_extracted.saturating_add(entry.header().size().unwrap_or(0));
            if total_extracted > MAX_EXTRACTED_BYTES {
                return Err(UpdateError::Install(format!(
                    "Archive decompressed size exceeds {MAX_EXTRACTED_BYTES} bytes — possible decompression bomb"
                )));
            }
            entry.unpack(&outpath)?;
        }

        self.find_binary_in_dir(extract_dir)
    }

    pub(crate) fn extract_zip(&self, archive_path: &Path) -> Result<PathBuf, UpdateError> {
        use std::fs::File;
        use std::io::Read;

        let file = File::open(archive_path)?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| UpdateError::Install(format!("Failed to open ZIP archive: {}", e)))?;

        let extract_dir = archive_path
            .parent()
            .unwrap_or(std::path::Path::new("/tmp"));

        let mut total_extracted: u64 = 0;
        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| UpdateError::Install(format!("Failed to read ZIP entry: {}", e)))?;

            let relative_path = file.enclosed_name().ok_or_else(|| {
                UpdateError::Install(format!("Unsafe ZIP entry path: {}", file.name()))
            })?;

            if !Self::is_safe_archive_path(&relative_path) {
                return Err(UpdateError::Install(format!(
                    "Unsafe ZIP entry path: {}",
                    file.name()
                )));
            }

            let outpath = extract_dir.join(relative_path);

            if file.name().ends_with('/') {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        std::fs::create_dir_all(p)?;
                    }
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                // Bound the actual decompressed copy: the zip reader yields the real
                // decompressed bytes, which a malicious local header can under-declare
                // in `file.size()`. `take(remaining + 1)` lets us detect an overflow.
                let remaining = MAX_EXTRACTED_BYTES.saturating_sub(total_extracted);
                let written = std::io::copy(&mut (&mut file).take(remaining + 1), &mut outfile)?;
                total_extracted = total_extracted.saturating_add(written);
                if total_extracted > MAX_EXTRACTED_BYTES {
                    return Err(UpdateError::Install(format!(
                        "Archive decompressed size exceeds {MAX_EXTRACTED_BYTES} bytes — possible decompression bomb"
                    )));
                }
            }
        }

        self.find_binary_in_dir(extract_dir)
    }

    pub(super) fn find_binary_in_dir(&self, dir: &std::path::Path) -> Result<PathBuf, UpdateError> {
        let binary_name = if cfg!(windows) {
            "maekon.exe"
        } else {
            "maekon"
        };

        let direct_path = dir.join(binary_name);
        if direct_path.exists() {
            return Ok(direct_path);
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let sub_binary = path.join(binary_name);
                if sub_binary.exists() {
                    return Ok(sub_binary);
                }
            } else if path.file_name().is_some_and(|n| n == binary_name) {
                return Ok(path);
            }
        }

        Err(UpdateError::Install(format!(
            "Binary '{}' not found",
            binary_name
        )))
    }
}
