//! Free-space check before large copies (backup, remote snapshot, compression).
use audiolib_core::{Error, Result};
use std::path::Path;

/// Slack on top of the requested bytes (journal, filesystem metadata, other writers).
const MARGIN_MIN: u64 = 64 << 20;

/// Free bytes available to this user on the volume holding `dir`; `None` where it cannot
/// be determined (non-Unix platforms, odd filesystems).
pub fn free_space(dir: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        // SAFETY: `path` is a valid NUL-terminated string and `stat` a zeroed out-parameter.
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
            return None;
        }
        Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        None
    }
}

/// Fail with an I/O error of kind `StorageFull` when the volume of `dir` has less than
/// `required` bytes (plus a margin of `max(64 MiB, 2 %)`) free. Unknown free space passes.
pub fn ensure_free_space(dir: &Path, required: u64) -> Result<()> {
    let Some(free) = free_space(dir) else {
        return Ok(());
    };
    let needed = required.saturating_add((required / 50).max(MARGIN_MIN));
    if free >= needed {
        return Ok(());
    }
    const MIB: u64 = 1 << 20;
    Err(Error::Io(std::io::Error::new(
        std::io::ErrorKind::StorageFull,
        format!(
            "Not enough free disk space in {}: about {} MiB needed, {} MiB free",
            dir.display(),
            needed.div_ceil(MIB),
            free / MIB
        ),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_request_passes_and_huge_one_reports_storage_full() {
        let dir = tempfile::tempdir().unwrap();
        ensure_free_space(dir.path(), 1024).unwrap();
        if free_space(dir.path()).is_some() {
            let error = ensure_free_space(dir.path(), u64::MAX / 2).unwrap_err();
            match error {
                Error::Io(io) => assert_eq!(io.kind(), std::io::ErrorKind::StorageFull),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}
