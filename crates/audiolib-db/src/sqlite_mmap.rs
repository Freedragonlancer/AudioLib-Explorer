// crates/audiolib-db/src/sqlite_mmap.rs
//! Conservative mapping policy for database readers; writers always use ordinary I/O.
use std::path::Path;

/// Maximum demand-paged mapping on a verified Windows fixed volume.
pub const SQLITE_READER_MMAP_BYTES: i64 = 68_719_476_736;

/// Requested mapping limit for a reader of this existing database.
///
/// Canonicalization resolves mapped drives, junctions and symlinks before volume
/// classification. Network, removable, unknown and unverified platforms use zero.
/// Setting `AUDIOLIB_DISABLE_MMAP` (to any value) before startup disables mapping
/// even on fixed volumes, including USB disks Windows classifies as fixed.
pub fn sqlite_reader_mmap_bytes(path: &Path) -> i64 {
    mapping_limit(std::env::var_os("AUDIOLIB_DISABLE_MMAP").is_some(), || {
        verified_fixed_volume(path)
    })
}

fn mapping_limit(disabled: bool, fixed_volume: impl FnOnce() -> bool) -> i64 {
    if !disabled && fixed_volume() {
        SQLITE_READER_MMAP_BYTES
    } else {
        0
    }
}

#[cfg(windows)]
fn verified_fixed_volume(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, Prefix};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetVolumePathNameW(file_name: *const u16, volume_path: *mut u16, capacity: u32) -> i32;
        fn GetDriveTypeW(root_path: *const u16) -> u32;
    }
    let is_network = |value: &Path| {
        matches!(value.components().next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _)))
    };
    // No network access is needed to reject an explicit UNC path.
    if is_network(path) {
        return false;
    }
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    if is_network(&canonical) {
        return false;
    }
    let encoded: Vec<u16> = canonical.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut volume = vec![0u16; 32_768];
    // SAFETY: the input is NUL-terminated, the output buffer has `capacity`
    // writable UTF-16 elements, and both live throughout the synchronous call.
    if unsafe { GetVolumePathNameW(encoded.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) }
        == 0
    {
        return false;
    }
    // SAFETY: a successful GetVolumePathNameW returns a NUL-terminated root in
    // `volume`; GetDriveTypeW only reads it during this synchronous call.
    unsafe {
        GetDriveTypeW(volume.as_ptr()) == 3 /* DRIVE_FIXED */
    }
}

#[cfg(not(windows))]
fn verified_fixed_volume(_path: &Path) -> bool {
    // MNT_LOCAL alone does not distinguish a Mac's removable local volumes.
    // Until that distinction is verified, prefer SQLite's recoverable xRead I/O.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonlocal_unknown_and_disabled_volumes_never_map() {
        assert_eq!(mapping_limit(false, || false), 0);
        assert_eq!(
            mapping_limit(true, || panic!("disabled means no volume probe")),
            0
        );
        assert_eq!(mapping_limit(false, || true), SQLITE_READER_MMAP_BYTES);
        assert_eq!(sqlite_reader_mmap_bytes(Path::new("missing-library.db")), 0);
    }

    #[cfg(windows)]
    #[test]
    fn network_paths_are_not_fixed_volumes() {
        // Volume classification of an unavailable UNC share must fail closed.
        assert!(!verified_fixed_volume(Path::new(
            r"\\localhost\nonexistent-audiolib-share\library.db"
        )));
    }

    #[test]
    fn writer_never_maps_and_pooled_readers_follow_the_volume_policy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mapping.db");
        let provider = crate::SqliteProvider::open(&path).unwrap();
        let writer = provider.writer_unlocked().unwrap();
        let writer_bytes: i64 = writer
            .pragma_query_value(None, "mmap_size", |row| row.get(0))
            .unwrap();
        assert_eq!(writer_bytes, 0);
        drop(writer);
        for _ in 0..2 {
            let reader = provider.reader().unwrap();
            let reader_bytes: i64 = reader
                .pragma_query_value(None, "mmap_size", |row| row.get(0))
                .unwrap();
            assert!((0..=sqlite_reader_mmap_bytes(&path)).contains(&reader_bytes));
        }
    }
}
