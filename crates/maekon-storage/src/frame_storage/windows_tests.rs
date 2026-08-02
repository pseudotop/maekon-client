use super::FrameFileStorage;
use std::path::Path;
use tempfile::TempDir;

async fn create_test_storage() -> (FrameFileStorage, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let storage = FrameFileStorage::new(temp_dir.path().to_path_buf(), 100, 7)
        .await
        .unwrap();
    (storage, temp_dir)
}

/// Reproduce the legacy `D:AI` state: a present, protected DACL with zero
/// ACEs. Enumeration and deletion remain denied until the owner replaces it.
fn set_empty_dacl(path: &Path) {
    use windows_sys::Win32::Security::Authorization::{SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        InitializeAcl, ACL, ACL_REVISION, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION,
    };

    let wide_path: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let acl_size = std::mem::size_of::<ACL>() as u32;
    let mut acl_buf = vec![0u8; acl_size as usize];
    let acl = acl_buf.as_mut_ptr().cast::<ACL>();

    unsafe {
        assert_ne!(
            InitializeAcl(acl, acl_size, ACL_REVISION),
            0,
            "empty ACL initialization must succeed"
        );
        assert_eq!(
            SetNamedSecurityInfoW(
                wide_path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null_mut(),
            ),
            0,
            "empty protected DACL must apply to the regression fixture"
        );
    }
}

/// #9276: retention must repair a legacy empty-DACL date directory and retry
/// deletion once instead of returning success while leaving captures behind.
#[tokio::test]
async fn enforce_retention_repairs_empty_dacl_and_deletes_expired_frames() {
    let (storage, _temp) = create_test_storage().await;
    let expired_dir = storage.frames_dir().join("2000-01-01");
    tokio::fs::create_dir_all(&expired_dir).await.unwrap();
    tokio::fs::write(expired_dir.join("12-00-00-0000000000.webp"), b"expired")
        .await
        .unwrap();
    set_empty_dacl(&expired_dir);

    let deleted = storage
        .enforce_retention()
        .await
        .expect("retention must self-heal the empty DACL");

    assert_eq!(deleted, 1);
    assert!(
        !expired_dir.exists(),
        "expired frame directory must be deleted after DACL repair"
    );
}
