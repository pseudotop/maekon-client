use parking_lot::Mutex as ParkingMutex;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

pub(super) const DISK_CHECK_INTERVAL: Duration = Duration::from_secs(5);
pub(super) const DISK_SPACE_WARN_MB: u64 = 100;
pub(super) const DISK_SPACE_CRITICAL_MB: u64 = 50;

pub struct DiskStatus {
    pub free_mb: u64,
    pub healthy: bool,
}

pub(super) struct DiskSpaceCache {
    last_check: ParkingMutex<Option<Instant>>,
    cached_free_mb: AtomicU64,
}

impl DiskSpaceCache {
    pub(super) fn new() -> Self {
        Self {
            last_check: ParkingMutex::new(None),
            cached_free_mb: AtomicU64::new(u64::MAX),
        }
    }

    pub(super) fn get_free_mb(&self, path: &Path) -> u64 {
        let mut last = self.last_check.lock();
        let now = Instant::now();
        if last.is_some_and(|t| now.duration_since(t) < DISK_CHECK_INTERVAL) {
            return self.cached_free_mb.load(AtomicOrdering::Relaxed);
        }
        let free = query_disk_free_mb(path);
        self.cached_free_mb.store(free, AtomicOrdering::Relaxed);
        *last = Some(now);
        free
    }
}

#[allow(clippy::unnecessary_cast)] // statvfs field types vary by platform
fn query_disk_free_mb(path: &Path) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return u64::MAX;
        };
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } == 0 {
            (stat.f_bavail as u64 * stat.f_frsize) / (1024 * 1024)
        } else {
            u64::MAX
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_bytes: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut free_bytes as *mut u64 as *mut _,
            )
        };
        if ok != 0 {
            free_bytes / (1024 * 1024)
        } else {
            u64::MAX
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        u64::MAX
    }
}
