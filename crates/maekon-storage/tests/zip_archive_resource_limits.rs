//! Security regressions for the local ZIP fork. Kept in one test because the
//! allocation meter is process-wide; fixture construction is outside each probe.
//! It measures requested live bytes, not RSS or allocator overhead. The temporary
//! old-plus-new storage inside System.realloc is outside this measurement.
// Test-only fixture builders use the workspace's configured unwrap allowance.
#![cfg(test)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use zip::result::ZipError;
use zip::{write::SimpleFileOptions, AesMode, CompressionMethod, ZipArchive, ZipWriter};

const MIB: usize = 1024 * 1024;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
struct AllocationMeter;
// SAFETY: every allocation operation delegates its pointer/layout contract to
// System unchanged. The atomics observe sizes and never inspect allocated memory.
unsafe impl GlobalAlloc for AllocationMeter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplies the layout required by GlobalAlloc.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(live, Ordering::SeqCst);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
        // SAFETY: the pointer and original layout are forwarded unchanged.
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: GlobalAlloc's caller guarantees a live pointer/layout and valid size.
        let new_pointer = unsafe { System.realloc(pointer, layout, size) };
        if !new_pointer.is_null() {
            let live = if size >= layout.size() {
                LIVE.fetch_add(size - layout.size(), Ordering::SeqCst) + size - layout.size()
            } else {
                LIVE.fetch_sub(layout.size() - size, Ordering::SeqCst) - (layout.size() - size)
            };
            PEAK.fetch_max(live, Ordering::SeqCst);
        }
        new_pointer
    }
}
#[global_allocator]
static ALLOCATOR: AllocationMeter = AllocationMeter;

struct CountedReader<'a> {
    inner: Cursor<&'a [u8]>,
    bytes_read: &'a mut usize,
}
impl Read for CountedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(buffer)?;
        *self.bytes_read += count;
        Ok(count)
    }
}
impl Seek for CountedReader<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}
fn parse_usage(bytes: &[u8]) -> (zip::result::ZipResult<usize>, usize, usize) {
    let mut read = 0;
    let baseline = LIVE.load(Ordering::SeqCst);
    PEAK.store(baseline, Ordering::SeqCst);
    let result = ZipArchive::new(CountedReader {
        inner: Cursor::new(bytes),
        bytes_read: &mut read,
    })
    .map(|archive| archive.len());
    let peak = PEAK.load(Ordering::SeqCst).saturating_sub(baseline);
    (result, read, peak)
}
fn put16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend(value.to_le_bytes());
}
fn put32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend(value.to_le_bytes());
}
fn put64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend(value.to_le_bytes());
}
fn stored_options() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
}
fn payload_zip(zip64: bool, signature_dense: bool) -> Vec<u8> {
    let mut payload = vec![0; 8 * MIB];
    if signature_dense {
        let signature = if zip64 { 0x0606_4b50_u32 } else { 0x0201_4b50 };
        for chunk in payload.as_chunks_mut::<4>().0 {
            chunk.copy_from_slice(&signature.to_le_bytes());
        }
    }
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("payload", stored_options()).unwrap();
    writer.write_all(&payload).unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    if zip64 {
        promote_to_zip64(bytes, 0)
    } else {
        bytes
    }
}
fn promote_to_zip64(mut bytes: Vec<u8>, extension: usize) -> Vec<u8> {
    let cd_size = u32::from_le_bytes(bytes[bytes.len() - 10..bytes.len() - 6].try_into().unwrap());
    let cd_start = u32::from_le_bytes(bytes[bytes.len() - 6..bytes.len() - 2].try_into().unwrap());
    bytes.truncate(bytes.len() - 22);
    let eocd64 = bytes.len() as u64;
    put32(&mut bytes, 0x0606_4b50);
    put64(&mut bytes, 44 + extension as u64);
    put16(&mut bytes, 45);
    put16(&mut bytes, 45);
    put32(&mut bytes, 0);
    put32(&mut bytes, 0);
    put64(&mut bytes, 1);
    put64(&mut bytes, 1);
    put64(&mut bytes, cd_size.into());
    put64(&mut bytes, cd_start.into());
    bytes.resize(bytes.len() + extension, 0x42);
    put32(&mut bytes, 0x0706_4b50);
    put32(&mut bytes, 0);
    put64(&mut bytes, eocd64);
    put32(&mut bytes, 1);
    put32(&mut bytes, 0x0605_4b50);
    put16(&mut bytes, 0);
    put16(&mut bytes, 0);
    put16(&mut bytes, u16::MAX);
    put16(&mut bytes, u16::MAX);
    put32(&mut bytes, u32::MAX);
    put32(&mut bytes, u32::MAX);
    put16(&mut bytes, 0);
    bytes
}
fn directory_candidates(headers: u16, candidates: u16, varying_offsets: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    for _ in 0..headers {
        put32(&mut bytes, 0x0201_4b50);
        bytes.extend([20, 0, 20, 0, 0, 0, 0, 0]);
        bytes.extend([0; 16]);
        bytes.extend([1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        bytes.extend([0; 8]);
        bytes.push(b'a');
    }
    let cd_size = bytes.len() as u32;
    for candidate in 0..candidates {
        put32(&mut bytes, 0x0605_4b50);
        bytes.extend([0; 4]);
        put16(&mut bytes, headers + 1);
        put16(&mut bytes, headers + 1);
        put32(&mut bytes, cd_size);
        put32(
            &mut bytes,
            if varying_offsets {
                u32::from(candidate) * 47 + 1
            } else {
                1
            },
        );
        put16(&mut bytes, 0);
    }
    bytes
}
fn zip64_candidates(count: usize, extension: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for _ in 0..count {
        put32(&mut bytes, 0x0606_4b50);
        put64(&mut bytes, 44 + extension as u64);
        put16(&mut bytes, 45);
        put16(&mut bytes, 45);
        put32(&mut bytes, 0);
        put32(&mut bytes, 1);
        for _ in 0..4 {
            put64(&mut bytes, 0);
        }
    }
    bytes.resize(bytes.len() + extension, 0);
    for _ in 0..count {
        put32(&mut bytes, 0x0706_4b50);
        put32(&mut bytes, 0);
        put64(&mut bytes, 0);
        put32(&mut bytes, 1);
        put32(&mut bytes, 0x0605_4b50);
        bytes.extend([0; 4]);
        put16(&mut bytes, u16::MAX);
        put16(&mut bytes, u16::MAX);
        put32(&mut bytes, u32::MAX);
        put32(&mut bytes, u32::MAX);
        put16(&mut bytes, 0);
    }
    bytes
}
fn bounded_detection() {
    for zip64 in [false, true] {
        for dense in [false, true] {
            for prefix in [false, true] {
                let mut bytes = payload_zip(zip64, dense);
                if prefix {
                    bytes.insert(0, 0x42);
                }
                let (result, read, peak) = parse_usage(&bytes);
                assert!(
                    peak < MIB,
                    "zip64={zip64}, dense={dense}, prefix={prefix}, peak={peak}"
                );
                if dense && prefix {
                    assert!(matches!(
                        result,
                        Err(zip::result::ZipError::UnsupportedArchive(_))
                    ));
                } else {
                    assert_eq!(result.unwrap(), 1);
                }
                if !prefix {
                    assert!(read < 16 * 1024, "ordinary metadata read payload: {read}");
                }
                assert!(
                    read < 2 * bytes.len() + 4096,
                    "offset detection reread payload: {read}"
                );
            }
        }
    }
    let bytes = directory_candidates(500, 500, true);
    let (result, read, peak) = parse_usage(&bytes);
    let error = result.unwrap_err();
    assert!(
        matches!(
            error,
            ZipError::UnsupportedArchive(
                "Archive offset detection exceeds the central-directory cache limit"
            )
        ),
        "overlapping directory chains: {error:?}"
    );
    assert!(
        peak < MIB,
        "overlapping directory chains allocated {peak} bytes"
    );
    assert!(read < MIB, "overlapping directory chains read {read} bytes");
    for (count, extension) in [(500, 0), (5, MIB)] {
        let bytes = zip64_candidates(count, extension);
        let (result, read, _) = parse_usage(&bytes);
        let error = result.unwrap_err();
        assert!(
            matches!(
                error,
                ZipError::UnsupportedArchive(
                    "Archive offset detection exceeds the ZIP64 candidate read limit"
                )
            ),
            "ZIP64 candidates: {error:?}"
        );
        assert!(
            read < 8 * bytes.len() + 4096,
            "ZIP64 candidates read {read} bytes from {}",
            bytes.len()
        );
    }
    // Same-offset candidates must remain linear, including the direct four-byte
    // checks for each plausible directory start. The unmodified upstream
    // 321-case complexity matrix is also part of the recorded vendor validation.
    let bytes = directory_candidates(256, 256, false);
    let (result, read, _) = parse_usage(&bytes);
    let error = result.unwrap_err();
    assert!(
        matches!(&error, ZipError::InvalidArchive(reason) if reason == "Invalid Central Directory header"),
        "same-offset candidates: {error:?}"
    );
    assert!(
        read <= 4 * bytes.len() + 1024,
        "same-offset candidates read {read} bytes"
    );
}
// Preserve the complete upstream eocd_complexity.rs input matrix and bound
// (zip2 commit 0123ed144cfa7d672dae78b79a034d8900f9aaeb). Only the shared
// fixture builder and counting reader replace upstream test scaffolding.
fn upstream_complexity_matrix() {
    for copies in [1, 2, 4, 8] {
        for headers in [0, 1, 2, 3, 4000, 8000, 16000, 32000, 64000] {
            for candidates in [0, 1, 2, 3, 4000, 8000, 16000, 32000, 64000] {
                if headers == 0 && candidates == 0 && copies > 1 {
                    continue;
                }
                let bytes = directory_candidates(headers, candidates, false).repeat(copies);
                let (result, read, _) = parse_usage(&bytes);
                let error = result.unwrap_err();
                match &error {
                    ZipError::InvalidArchive(reason) => assert!(
                        matches!(
                            reason.as_ref(),
                            "Could not find EOCD"
                                | "Invalid CDFH offset in EOCD"
                                | "No CDFH found"
                                | "Unexpected end of zip::format::blocks::ZipCentralEntryBlock"
                                | "Invalid Central Directory header"
                        ),
                        "unexpected malformed-directory error: {error:?}"
                    ),
                    ZipError::UnsupportedArchive(reason) => assert!(
                        matches!(
                            *reason,
                            "Archive offset detection exceeds the central-directory cache limit"
                                | "Archive offset detection exceeds the signature cache limit"
                        ),
                        "unexpected detection-limit error: {error:?}"
                    ),
                    _ => panic!("unexpected complexity-matrix error: {error:?}"),
                }
                assert!(
                    read <= 302 * bytes.len() / 100 + 1024,
                    "upstream matrix: copies={copies}, headers={headers}, candidates={candidates}, read={read}"
                );
            }
        }
    }
}
fn valid_large_archives() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..65_537 {
        writer
            .start_file(format!("entry-{index}"), stored_options())
            .unwrap();
    }
    let mut archive = ZipArchive::new(writer.finish().unwrap()).unwrap();
    assert_eq!(archive.len(), 65_537);
    assert_eq!(archive.by_index(0).unwrap().name().unwrap(), "entry-0");
    assert_eq!(
        archive
            .by_name("entry-65536")
            .unwrap()
            .read(&mut [0])
            .unwrap(),
        0
    );
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("empty", stored_options()).unwrap();
    let bytes = promote_to_zip64(writer.finish().unwrap().into_inner(), MIB);
    assert_eq!(ZipArchive::new(Cursor::new(bytes)).unwrap().len(), 1);
}
// A fake EOCD in an otherwise valid comment exercises the documented fallback
// boundary separately from first-valid large-directory support.
fn comment_candidate_boundary() {
    for entries in [2_u16, 9_000] {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for index in 0..entries {
            writer
                .start_file(format!("entry-{index}"), stored_options())
                .unwrap();
        }
        let mut bytes = writer.finish().unwrap().into_inner();
        let eocd = bytes.len() - 22;
        let mut fake = bytes[eocd..].to_vec();
        fake[8..10].copy_from_slice(&(entries + 1).to_le_bytes());
        fake[10..12].copy_from_slice(&(entries + 1).to_le_bytes());
        let size = u32::from_le_bytes(fake[12..16].try_into().unwrap());
        fake[12..16].copy_from_slice(&(size + 22).to_le_bytes());
        bytes[eocd + 20..eocd + 22].copy_from_slice(&22_u16.to_le_bytes());
        bytes.extend(fake);
        let result = ZipArchive::new(Cursor::new(bytes));
        if entries == 2 {
            assert_eq!(result.unwrap().len(), usize::from(entries));
        } else {
            assert!(matches!(
                result,
                Err(zip::result::ZipError::UnsupportedArchive(_))
            ));
        }
    }
}
fn abort_comment_roundtrip() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.set_comment("archive comment").unwrap();
    writer.start_file("keep", stored_options()).unwrap();
    writer.write_all(b"kept").unwrap();
    writer.start_file("discard", stored_options()).unwrap();
    writer.write_all(&[0x42; 4096]).unwrap();
    writer.abort_file().unwrap();
    let mut archive = writer.finish_into_readable().unwrap();
    assert_eq!(archive.comment(), b"archive comment");
    assert_eq!(archive.len(), 1);
    let mut contents = Vec::new();
    archive
        .by_name("keep")
        .unwrap()
        .read_to_end(&mut contents)
        .unwrap();
    assert_eq!(contents, b"kept");
    let reopened = ZipArchive::new(archive.into_inner()).unwrap();
    assert_eq!(reopened.comment(), b"archive comment");
    let mut append = ZipWriter::new_append(reopened.into_inner()).unwrap();
    append.start_file("after", stored_options()).unwrap();
    append.write_all(b"after").unwrap();
    let mut archive = ZipArchive::new(append.finish().unwrap()).unwrap();
    assert_eq!(archive.len(), 2);
    let mut contents = Vec::new();
    archive
        .by_name("after")
        .unwrap()
        .read_to_end(&mut contents)
        .unwrap();
    assert_eq!(contents, b"after");
}
fn aes_empty_read_preserves_authentication() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "aes",
            stored_options().with_aes_encryption(AesMode::Aes256, "test password"),
        )
        .unwrap();
    writer.write_all(b"encrypted payload").unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let mut archive = ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
    let mut entry = archive.by_name_decrypt("aes", b"test password").unwrap();
    assert_eq!(entry.read(&mut []).unwrap(), 0);
    let mut payload = Vec::new();
    entry.read_to_end(&mut payload).unwrap();
    assert_eq!(payload, b"encrypted payload");
    let data_end = entry.data_start().unwrap() as usize + entry.compressed_size() as usize;
    drop(entry);
    let mut tampered = bytes;
    tampered[data_end - 1] ^= 1;
    let mut archive = ZipArchive::new(Cursor::new(tampered)).unwrap();
    let mut entry = archive.by_name_decrypt("aes", b"test password").unwrap();
    assert_eq!(entry.read(&mut []).unwrap(), 0);
    let error = entry.read_to_end(&mut Vec::new()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        error.to_string().contains("Invalid authentication code"),
        "empty read bypassed AES authentication: {error}"
    );
}
#[cfg(unix)]
fn symlink_creation_preserves_target() {
    use std::os::unix::fs::PermissionsExt;
    use std::{fs, path::PathBuf};
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    fs::write(&target, b"private").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_symlink("link", "target", stored_options().unix_permissions(0o777))
        .unwrap();
    writer
        .finish_into_readable()
        .unwrap()
        .extract(root.path())
        .unwrap();
    let link = root.path().join("link");
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&link).unwrap(), PathBuf::from("target"));
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    assert_eq!(fs::read(&target).unwrap(), b"private");
}
#[cfg(unix)]
fn deferred_permissions_do_not_follow_symlinks() {
    use std::cell::Cell;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::{fs, path::Path};

    // A queued directory is replaced while the next entry's payload is read.
    // This deterministically exercises the deferred-permissions helper with a
    // symlink operand; a ZIP containing only a symlink never calls that helper.
    struct ReplaceQueuedDirectory<'a> {
        inner: Cursor<Vec<u8>>,
        data_start: u64,
        directory: &'a Path,
        armed: &'a Cell<bool>,
        fired: &'a Cell<usize>,
    }
    impl Read for ReplaceQueuedDirectory<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let position = self.inner.position();
            let count = self.inner.read(buffer)?;
            if self.armed.get() && count > 0 && position == self.data_start {
                assert_eq!(self.fired.get(), 0, "payload hook fired more than once");
                assert!(fs::symlink_metadata(self.directory)?.file_type().is_dir());
                assert!(fs::read_dir(self.directory)?.next().is_none());
                fs::remove_dir(self.directory)?;
                symlink("target", self.directory)?;
                self.fired.set(1);
            }
            Ok(count)
        }
    }
    impl Seek for ReplaceQueuedDirectory<'_> {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.inner.seek(position)
        }
    }

    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    fs::write(&target, b"private").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_directory("queued/", stored_options().unix_permissions(0o711))
        .unwrap();
    writer.start_file("trigger", stored_options()).unwrap();
    writer.write_all(b"last entry payload").unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let data_start = ZipArchive::new(Cursor::new(&bytes))
        .unwrap()
        .by_name("trigger")
        .unwrap()
        .data_start()
        .unwrap();
    let link = root.path().join("queued");
    let armed = Cell::new(false);
    let fired = Cell::new(0);
    let mut archive = ZipArchive::new(ReplaceQueuedDirectory {
        inner: Cursor::new(bytes),
        data_start,
        directory: &link,
        armed: &armed,
        fired: &fired,
    })
    .unwrap();
    armed.set(true);
    archive.extract(root.path()).unwrap();
    assert_eq!(
        fired.get(),
        1,
        "deferred-permissions hook was not exercised"
    );
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("target"));
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
        0o600,
        "deferred chmod followed the symlink"
    );
    assert_eq!(fs::read(&target).unwrap(), b"private");
    assert_eq!(
        fs::read(root.path().join("trigger")).unwrap(),
        b"last entry payload"
    );
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    assert_eq!(
        fs::symlink_metadata(&link).unwrap().permissions().mode() & 0o7777,
        0o711,
        "deferred chmod did not update the symlink's nondefault mode"
    );
}
#[test]
fn zip_archive_security_regressions() {
    bounded_detection();
    upstream_complexity_matrix();
    valid_large_archives();
    comment_candidate_boundary();
    abort_comment_roundtrip();
    aes_empty_read_preserves_authentication();
    #[cfg(unix)]
    {
        symlink_creation_preserves_target();
        deferred_permissions_do_not_follow_symlinks();
    }
}
