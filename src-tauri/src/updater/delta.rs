use super::UpdateError;

/// Apply a bsdiff patch to the old binary, producing the new binary.
pub fn apply_patch(old_binary: &[u8], patch_data: &[u8]) -> Result<Vec<u8>, UpdateError> {
    let mut new_binary: Vec<u8> = Vec::new();
    bsdiff::patch(
        old_binary,
        &mut std::io::Cursor::new(patch_data),
        &mut new_binary,
    )
    .map_err(|e| UpdateError::Install(format!("bsdiff patch failed: {e}")))?;
    Ok(new_binary)
}

/// Resolve the path of the currently running binary.
pub fn current_binary_path() -> Result<std::path::PathBuf, UpdateError> {
    std::env::current_exe()
        .map_err(|e| UpdateError::Install(format!("Cannot determine current binary path: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_patch_roundtrip() {
        let old = b"Hello, World!";
        let new_expected = b"Hello, Rust!";
        let mut patch_data = Vec::new();
        bsdiff::diff(old, new_expected, &mut patch_data).unwrap();
        let result = apply_patch(old, &patch_data).unwrap();
        assert_eq!(result, new_expected);
    }

    #[test]
    fn apply_patch_identical() {
        let data = b"No changes here";
        let mut patch_data = Vec::new();
        bsdiff::diff(data, data, &mut patch_data).unwrap();
        let result = apply_patch(data, &patch_data).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn apply_patch_invalid_data_returns_error() {
        let old = b"original";
        let bad_patch = b"not a valid bsdiff patch";
        assert!(apply_patch(old, bad_patch).is_err());
    }

    #[test]
    fn current_binary_path_returns_path() {
        let path = current_binary_path().unwrap();
        assert!(path.exists());
    }
}
