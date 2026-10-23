// crates/audiolib-remote/src/cache_cleanup.rs
//! Conservative housekeeping for owned remote snapshot staging files.
use crate::error::{RemoteError, Result};
use crate::sync::{CacheState, RemoteCache, directory_name};
use std::fs::{File, Metadata, OpenOptions};
use std::path::Path;
use std::time::Duration;

/// Failed preparations may leave large output files behind after an abrupt exit.
pub(crate) const STALE_STAGING_AGE: Duration = Duration::from_secs(60 * 60);

fn plain_metadata(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions and other reparse points must not redirect housekeeping.
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}

pub(crate) fn plain_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && plain_metadata(&metadata))
}

/// Housekeeping must not cross a junction hidden above the selected cache root.
pub(crate) fn plain_directory_tree(path: &Path) -> bool {
    path.ancestors()
        .filter(|ancestor| !ancestor.as_os_str().is_empty())
        .all(plain_directory)
}

fn plain_file(path: &Path) -> Option<Metadata> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file() && plain_metadata(metadata))
}

fn missing_or_plain_file(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata.is_file() && plain_metadata(&metadata),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

fn numeric_parts(value: &str, min: usize, max: usize) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    (min..=max).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn snapshot_name(name: &str) -> bool {
    name.strip_prefix("library-r")
        .and_then(|name| name.strip_suffix(".db"))
        .is_some_and(|name| numeric_parts(name, 2, 3))
}

fn staging_name(name: &str) -> bool {
    if let Some(snapshot) = name.strip_suffix(".download") {
        return snapshot_name(snapshot);
    }
    name.rsplit_once(".part-").is_some_and(|(base, pid)| {
        numeric_parts(pid, 1, 1)
            && (snapshot_name(base) || base.strip_suffix(".download").is_some_and(snapshot_name))
    })
}

fn state_temporary_name(name: &str) -> bool {
    name.strip_prefix("cache.tmp-")
        .is_some_and(|parts| numeric_parts(parts, 2, 2))
}

fn stale(metadata: &Metadata) -> bool {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > STALE_STAGING_AGE)
}

/// Excludes housekeeping while an owned snapshot is being built or installed.
/// Unlike publication, this lease spans network/SQLite work; the short publication
/// lease may be acquired while it is held. Retain the lock file after unlocking.
pub(crate) struct SnapshotStagingLease {
    _file: File,
}

impl Drop for SnapshotStagingLease {
    fn drop(&mut self) {
        // Keep the lock file: replacing it would split exclusion across handles.
        let _ = self._file.unlock();
    }
}

impl SnapshotStagingLease {
    pub(crate) fn acquire(cache: &RemoteCache, library_id: &str) -> Result<Self> {
        std::fs::create_dir_all(cache.root())?;
        let directory = cache.root().join(".staging-locks");
        if !plain_directory(cache.root())
            || std::fs::symlink_metadata(&directory)
                .is_ok_and(|metadata| !metadata.is_dir() || !plain_metadata(&metadata))
        {
            return Err(RemoteError::Validation(
                "Snapshot staging lock directory must not be linked".into(),
            ));
        }
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{}.lock", directory_name(library_id)));
        if std::fs::symlink_metadata(&path)
            .is_ok_and(|metadata| !metadata.is_file() || !plain_metadata(&metadata))
        {
            return Err(RemoteError::Validation(
                "Snapshot staging lock must be a regular file".into(),
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock().map_err(|_| RemoteError::Busy {
            message: "Another process is preparing or cleaning this library cache".into(),
            retry_after: None,
        })?;
        Ok(Self { _file: file })
    }
}

struct CleanupLease {
    _publication: crate::writes::CachePublicationLease,
    _staging: SnapshotStagingLease,
}

fn cleanup_lease(cache: &RemoteCache, library_id: &str) -> Option<CleanupLease> {
    let directory = cache.library_dir(library_id);
    if !plain_directory_tree(cache.root()) || !plain_directory(&directory) {
        return None;
    }
    if !missing_or_plain_file(&cache.root().join("active.json"))
        || !missing_or_plain_file(&directory.join("cache.json"))
    {
        return None;
    }
    // The publication helper is also used by ordinary writes. Housekeeping applies
    // the stricter no-reparse rule before it can open any shared lock namespace.
    let publication = cache.root().join(".publication-locks");
    for path in [
        publication.clone(),
        publication.join(format!("{}.lock", directory_name(library_id))),
    ] {
        if std::fs::symlink_metadata(&path).is_ok_and(|metadata| !plain_metadata(&metadata)) {
            return None;
        }
    }
    // Every cleanup entry point acquires each lease once and never waits on the
    // process-wide publication mutex. Normal preparations use this same order.
    let staging = SnapshotStagingLease::acquire(cache, library_id).ok()?;
    let publication = crate::writes::try_cache_publication_lease(cache, library_id).ok()?;
    Some(CleanupLease {
        _publication: publication,
        _staging: staging,
    })
}

/// Only generated snapshot names and atomic cache-state temporaries are eligible.
/// `cache.json` is reread under publication exclusion so a newly installed snapshot
/// is retained even if the caller captured an older keep list. Recovery jobs are not
/// traversed, regardless of age; durable wire transfers live outside this directory.
pub(crate) fn prune_files(
    cache: &RemoteCache,
    library_id: &str,
    keep: &[&str],
    snapshots: bool,
) -> usize {
    let Some(_leases) = cleanup_lease(cache, library_id) else {
        return 0;
    };
    let directory = cache.library_dir(library_id);
    let recorded = if directory.join("cache.json").exists() {
        let Some(state) = recorded_state(&directory).filter(|state| {
            state.library_id == library_id && !state.database_file.contains(['/', '\\'])
        }) else {
            return 0;
        };
        Some(state.database_file)
    } else {
        None
    };
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return 0;
    };
    let Ok(entries) = entries.collect::<std::io::Result<Vec<_>>>() else {
        return 0;
    };
    let recovery = entries.iter().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .starts_with(".snapshot-job-")
    });
    let mut removed = 0;
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let base = name
            .strip_suffix("-journal")
            .or_else(|| name.strip_suffix("-wal"))
            .or_else(|| name.strip_suffix("-shm"))
            .unwrap_or(&name);
        if keep.contains(&base) || recorded.as_deref() == Some(base) {
            continue;
        }
        let path = entry.path();
        let Some(metadata) = plain_file(&path) else {
            continue;
        };
        let remove = if staging_name(base) || state_temporary_name(&name) {
            // A fresh staging database also protects its older SQLite sidecars.
            stale(&metadata)
                && (base == name
                    || plain_file(&directory.join(base)).is_none_or(|metadata| stale(&metadata)))
        } else {
            snapshots && !recovery && snapshot_name(base)
        };
        if remove && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

fn recorded_state(directory: &Path) -> Option<CacheState> {
    plain_file(&directory.join("cache.json"))?;
    serde_json::from_slice(&std::fs::read(directory.join("cache.json")).ok()?).ok()
}

/// Remove only identified mirror files of a replaced library. Unknown files, manual
/// backups and recovery jobs keep the directory/state available for inspection.
pub(crate) fn discard_replaced(cache: &RemoteCache, state: &CacheState) -> bool {
    let Some(_leases) = cleanup_lease(cache, &state.library_id) else {
        return false;
    };
    let directory = cache.library_dir(&state.library_id);
    if cache.root().join("active.json").exists() && cache.active_library_id().is_none() {
        return false;
    }
    if cache.active_library_id().as_deref() == Some(state.library_id.as_str())
        || recorded_state(&directory).as_ref() != Some(state)
    {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return false;
    };
    let Ok(entries) = entries.collect::<std::io::Result<Vec<_>>>() else {
        return false;
    };
    // An interrupted protected migration may still depend on the recorded source.
    // Preserve its whole library instead of guessing from the job's age/content.
    if entries.iter().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .starts_with(".snapshot-job-")
    }) {
        return false;
    }
    let mut clean = true;
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "cache.json" {
            continue;
        }
        let base = name
            .strip_suffix("-journal")
            .or_else(|| name.strip_suffix("-wal"))
            .or_else(|| name.strip_suffix("-shm"))
            .unwrap_or(&name);
        let path = entry.path();
        if name == "audio-cache" {
            clean &= remove_audition_cache(&path);
        } else if let Some(metadata) = plain_file(&path) {
            let generated = snapshot_name(base) || base == state.database_file;
            let temporary = (staging_name(base) || state_temporary_name(&name))
                && stale(&metadata)
                && (base == name
                    || plain_file(&directory.join(base)).is_none_or(|metadata| stale(&metadata)));
            clean &= (generated || temporary) && std::fs::remove_file(path).is_ok();
        } else {
            clean = false;
        }
    }
    clean
        && std::fs::remove_file(directory.join("cache.json")).is_ok()
        && std::fs::remove_dir(directory).is_ok()
}

/// The obsolete audition cache has one flat generated namespace. Skip unknown or
/// linked entries rather than recursively following user-created subdirectories.
pub(crate) fn remove_audition_cache(directory: &Path) -> bool {
    if !plain_directory_tree(directory) {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    let mut clean = true;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let base = name.strip_suffix(".etag").unwrap_or(&name);
        let base = match base.split_once(".part-") {
            Some((base, suffix)) if numeric_parts(suffix, 1, 3) => base,
            Some(_) => {
                clean = false;
                continue;
            }
            None => base,
        };
        let generated = base.strip_suffix(".wav").is_some_and(|stem| {
            stem.split_once('-').is_some_and(|(id, key)| {
                !id.is_empty()
                    && id.bytes().all(|byte| byte.is_ascii_digit())
                    && !key.is_empty()
                    && key
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        });
        clean &= generated
            && plain_file(&entry.path()).is_some()
            && std::fs::remove_file(entry.path()).is_ok();
    }
    clean && std::fs::remove_dir(directory).is_ok()
}

impl RemoteCache {
    /// Remove generated staging/state temporary files untouched for over an hour in
    /// every owned library directory, including abandoned first-sync directories.
    /// Current and old completed snapshots are retained until a successful provider
    /// swap. Active preparation leases, linked paths and unknown files are skipped.
    /// Durable `.transfers` payloads/checkpoints and manual backups are never visited.
    pub fn cleanup_stale_temporaries(&self) -> usize {
        if !plain_directory_tree(self.root()) {
            return 0;
        }
        let Ok(entries) = std::fs::read_dir(self.root()) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let directory = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "placeholder" || !plain_directory(&directory) {
                continue;
            }
            let recorded: Option<CacheState> = plain_file(&directory.join("cache.json"))
                .and_then(|_| std::fs::read(directory.join("cache.json")).ok())
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            let library_id = match recorded {
                Some(state) if self.library_dir(&state.library_id) == directory => state.library_id,
                None if directory_name(&name) == name => name,
                _ => continue,
            };
            removed += prune_files(self, &library_id, &[], false);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old_file(path: &Path) {
        std::fs::write(path, b"owned fixture").unwrap();
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(
                std::time::SystemTime::now() - STALE_STAGING_AGE - Duration::from_secs(60),
            )
            .unwrap();
    }

    #[test]
    fn startup_removes_only_stale_generated_temporaries_across_libraries() {
        let root = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(root.path());
        for id in ["active", "inactive", "abandoned-first-sync"] {
            let directory = cache.library_dir(id);
            std::fs::create_dir_all(&directory).unwrap();
            for name in ["library-r1-1.db.download", "cache.tmp-123-456"] {
                old_file(&directory.join(name));
            }
            for name in [
                "library-r1-1.db",
                "library-r2-2.db",
                "library-rmanual.db",
                "library-r1-1.db.backup",
                "library-r1-1.db.download.notes",
                "backup.db",
            ] {
                old_file(&directory.join(name));
            }
            std::fs::write(directory.join("library-r3-3.db.download"), b"fresh").unwrap();
        }
        let transfer = cache.root().join(".transfers").join("endpoint");
        std::fs::create_dir_all(&transfer).unwrap();
        old_file(&transfer.join("payload.part"));
        old_file(&transfer.join("checkpoint.json"));
        assert_eq!(cache.cleanup_stale_temporaries(), 6);
        for id in ["active", "inactive", "abandoned-first-sync"] {
            let directory = cache.library_dir(id);
            assert!(!directory.join("library-r1-1.db.download").exists());
            assert!(!directory.join("cache.tmp-123-456").exists());
            assert_eq!(std::fs::read_dir(directory).unwrap().count(), 7);
        }
        assert!(transfer.join("payload.part").exists());
        assert!(transfer.join("checkpoint.json").exists());
    }

    #[test]
    fn active_preparation_and_recorded_snapshot_survive_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(root.path());
        let directory = cache.library_dir("lib");
        std::fs::create_dir_all(&directory).unwrap();
        let staging = directory.join("library-r1-1.db.download");
        old_file(&staging);
        let lease = SnapshotStagingLease::acquire(&cache, "lib").unwrap();
        assert_eq!(cache.cleanup_stale_temporaries(), 0);
        assert!(staging.exists());
        drop(lease);
        assert_eq!(cache.cleanup_stale_temporaries(), 1);
        let recorded = CacheState {
            library_id: "lib".into(),
            database_file: "library-r2-2.db".into(),
            ..Default::default()
        };
        old_file(&cache.database_path(&recorded));
        old_file(&directory.join("library-r2-2.db-wal"));
        old_file(&directory.join("library-r2-2.db-shm"));
        old_file(&directory.join("library-r1-1.db"));
        cache.save_state(&recorded).unwrap();
        cache.prune("lib", &[]);
        assert!(cache.database_path(&recorded).exists());
        assert!(directory.join("library-r2-2.db-wal").exists());
        assert!(directory.join("library-r2-2.db-shm").exists());
        assert!(!directory.join("library-r1-1.db").exists());
    }

    #[test]
    fn fresh_staging_protects_its_older_sidecar() {
        let root = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(root.path());
        let directory = cache.library_dir("lib");
        std::fs::create_dir_all(&directory).unwrap();
        old_file(&directory.join("library-r1-1.db.download-wal"));
        std::fs::write(directory.join("library-r1-1.db.download"), b"fresh").unwrap();
        assert_eq!(cache.cleanup_stale_temporaries(), 0);
        assert!(directory.join("library-r1-1.db.download-wal").exists());
    }

    #[test]
    fn recovery_jobs_manual_backups_and_durable_transfers_are_retained() {
        let root = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(root.path());
        let old = CacheState {
            library_id: "old".into(),
            database_file: "library-r1-1.db".into(),
            server_url: "http://example.invalid/api/v1".into(),
            ..Default::default()
        };
        let active = CacheState {
            library_id: "current".into(),
            ..old.clone()
        };
        for state in [&old, &active] {
            std::fs::create_dir_all(cache.library_dir(&state.library_id)).unwrap();
            old_file(&cache.database_path(state));
            cache.save_state(state).unwrap();
        }
        let directory = cache.library_dir("old");
        let job = directory.join(".snapshot-job-abandoned");
        std::fs::create_dir_all(job.join("protected-backup")).unwrap();
        old_file(&job.join("snapshot.download"));
        old_file(&job.join("protected-backup/rollback.db"));
        let backup = directory.join("library-r1-1.db.backup");
        old_file(&backup);
        let prior_source = directory.join("library-r0-1.db");
        old_file(&prior_source);
        old_file(&directory.join("library-r2-2.db.download"));
        old_file(&directory.join("cache.tmp-3-4"));
        let transfer = cache.root().join(".transfers/endpoint");
        std::fs::create_dir_all(&transfer).unwrap();
        old_file(&transfer.join("payload.part"));
        old_file(&transfer.join("checkpoint.json"));
        assert_eq!(cache.cleanup_stale_temporaries(), 2);
        cache.prune("old", &[]);
        assert!(cache.discard_replaced(&active).is_empty());
        assert!(job.join("snapshot.download").is_file());
        assert!(job.join("protected-backup/rollback.db").is_file());
        assert!(backup.is_file());
        assert!(
            prior_source.is_file(),
            "recovery may depend on an older mirror"
        );
        assert!(cache.database_path(&old).is_file());
        assert!(cache.database_path(&active).is_file());
        assert!(transfer.join("payload.part").is_file());
        assert!(transfer.join("checkpoint.json").is_file());
        assert_eq!(cache.active().unwrap(), active);
    }

    #[test]
    fn replaced_mirror_cleanup_keeps_unknown_files_and_manual_backups() {
        let root = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(root.path());
        let state = CacheState {
            library_id: "old".into(),
            database_file: "library-r1-1.db".into(),
            ..Default::default()
        };
        let directory = cache.library_dir("old");
        std::fs::create_dir_all(&directory).unwrap();
        old_file(&cache.database_path(&state));
        cache.save_state(&state).unwrap();
        // The exact active identity must be rechecked while the leases are held.
        assert!(!discard_replaced(&cache, &state));
        let mut replacement = state.clone();
        replacement.library_id = "new".into();
        std::fs::create_dir_all(cache.library_dir("new")).unwrap();
        old_file(&cache.database_path(&replacement));
        cache.save_state(&replacement).unwrap();
        for name in [
            "backup.db",
            "library-r1-backup.db",
            "library-r1-1.db.backup",
        ] {
            old_file(&directory.join(name));
        }
        assert!(!discard_replaced(&cache, &state));
        assert!(!cache.database_path(&state).exists());
        assert!(directory.join("cache.json").is_file());
        for name in [
            "backup.db",
            "library-r1-backup.db",
            "library-r1-1.db.backup",
        ] {
            assert!(directory.join(name).is_file());
        }
        assert_eq!(cache.active().unwrap(), replacement);
    }

    #[test]
    fn staging_lease_child() {
        let Some(root) = std::env::var_os("AUDIOLIB_CLEANUP_CHILD_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let cache = RemoteCache::new(&root);
        let _lease = SnapshotStagingLease::acquire(&cache, "lib").unwrap();
        std::fs::write(root.join("child-ready"), b"ready").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !root.join("child-release").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn another_process_preparation_excludes_cleanup_and_fails_sync_admission_fast() {
        use crate::test_server::{TestResponse, TestServer};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        let root = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(root.path());
        let state = CacheState {
            library_id: "lib".into(),
            revision: 7,
            schema_version: 5,
            database_file: "library-r7-1.db".into(),
            ..Default::default()
        };
        std::fs::create_dir_all(cache.library_dir("lib")).unwrap();
        let connection = rusqlite::Connection::open(cache.database_path(&state)).unwrap();
        audiolib_db::register_library_functions(&connection).unwrap();
        connection
            .execute_batch(include_str!(
                "../../audiolib-db/tests/fixtures/schema-v5.sql"
            ))
            .unwrap();
        connection
            .execute(
                "DELETE FROM audiolib_schema_features WHERE feature='tokenization'",
                [],
            )
            .unwrap();
        drop(connection);
        cache.save_state(&state).unwrap();
        let staging = cache.library_dir("lib").join("library-r8-1.db.download");
        old_file(&staging);
        let downloads = Arc::new(AtomicUsize::new(0));
        let observed = downloads.clone();
        let server = TestServer::start(move |request| match request.path.as_str() {
            "/api/v1/health" => crate::test_server::marker_health(),
            "/api/v1/capabilities" => {
                crate::test_server::marker_capabilities(serde_json::json!({}))
            }
            "/api/v1/library" => TestResponse::json(
                200,
                r#"{"library_id":"lib","revision":8,"schema_version":5}"#,
            ),
            "/api/v1/roots" => TestResponse::json(200, "[]"),
            _ => {
                observed.fetch_add(1, Ordering::Relaxed);
                TestResponse::json(500, "{}")
            }
        });
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "cache_cleanup::tests::staging_lease_child",
                "--nocapture",
            ])
            .env("AUDIOLIB_CLEANUP_CHILD_ROOT", root.path());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().unwrap();
        let ready_deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !root.path().join("child-ready").exists()
            && std::time::Instant::now() < ready_deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let ready = root.path().join("child-ready").exists();
        let started = std::time::Instant::now();
        let prepared = cache.prepare_cached_state(&state);
        let prepare_elapsed = started.elapsed();
        let started = std::time::Instant::now();
        let synced = crate::sync_library(
            &crate::RemoteClient::new(&server.url, "").unwrap(),
            &cache,
            &mut |_| {},
            &AtomicBool::new(false),
            true,
        );
        let sync_elapsed = started.elapsed();
        let removed = cache.cleanup_stale_temporaries();
        cache.prune("lib", &[]);
        let retained = staging.exists() && cache.database_path(&state).exists();
        std::fs::write(root.path().join("child-release"), b"release").unwrap();
        let status = child.wait().unwrap();
        assert!(ready && status.success());
        assert!(
            matches!(prepared, Err(RemoteError::Busy { .. })),
            "{prepared:?}"
        );
        assert!(
            matches!(synced, Err(RemoteError::Busy { .. })),
            "{synced:?}"
        );
        assert!(
            prepare_elapsed < Duration::from_secs(2),
            "{prepare_elapsed:?}"
        );
        assert!(sync_elapsed < Duration::from_secs(2), "{sync_elapsed:?}");
        assert_eq!(downloads.load(Ordering::Relaxed), 0);
        assert_eq!(removed, 0);
        assert!(retained);
        assert_eq!(
            cache.cleanup_stale_temporaries(),
            1,
            "explicit unlock allows immediate cleanup"
        );
    }

    #[cfg(windows)]
    #[test]
    fn junctions_in_roots_ancestors_library_and_lock_paths_are_not_followed() {
        use std::os::windows::process::CommandExt;
        for case in [
            "root",
            "ancestor",
            "library",
            "staging",
            "publication",
            "staging-leaf",
            "publication-leaf",
        ] {
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let data_root = if case == "ancestor" {
                root.path().join("link/nested")
            } else {
                root.path().join("data")
            };
            let cache = RemoteCache::new(&data_root);
            let linked = match case {
                "root" => cache.root().to_path_buf(),
                "ancestor" => root.path().join("link"),
                "library" => cache.library_dir("lib"),
                "staging" => cache.root().join(".staging-locks"),
                "publication" => cache.root().join(".publication-locks"),
                "staging-leaf" => cache.root().join(".staging-locks").join("lib.lock"),
                _ => cache.root().join(".publication-locks").join("lib.lock"),
            };
            std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
            assert!(linked.starts_with(root.path()));
            let status = std::process::Command::new("cmd.exe")
                .args(["/c", "mklink", "/J"])
                .arg(&linked)
                .arg(outside.path())
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(status.status.success(), "{case}: {status:?}");
            let old = CacheState {
                library_id: "lib".into(),
                database_file: "library-r1-1.db".into(),
                server_url: "http://example.invalid/api/v1".into(),
                ..Default::default()
            };
            let active = CacheState {
                library_id: "new".into(),
                ..old.clone()
            };
            for state in [&old, &active] {
                std::fs::create_dir_all(cache.library_dir(&state.library_id)).unwrap();
                old_file(&cache.database_path(state));
                cache.save_state(state).unwrap();
            }
            let staging = cache.library_dir("lib").join("library-r1-1.db.download");
            old_file(&staging);
            old_file(&outside.path().join("sentinel"));
            if matches!(case, "root" | "ancestor" | "library") {
                let audio = cache.audio_cache_dir("lib");
                std::fs::create_dir_all(&audio).unwrap();
                old_file(&audio.join("7-x.wav"));
                assert_eq!(cache.remove_audition_caches(), 0, "{case}");
                assert!(!remove_audition_cache(&audio), "{case}");
                assert!(audio.join("7-x.wav").is_file(), "{case}");
            }
            assert_eq!(cache.cleanup_stale_temporaries(), 0, "{case}");
            cache.prune("lib", &[]);
            assert!(cache.discard_replaced(&active).is_empty(), "{case}");
            assert!(staging.exists(), "{case}");
            assert!(cache.database_path(&old).exists(), "{case}");
            assert_eq!(
                std::fs::read(outside.path().join("sentinel")).unwrap(),
                b"owned fixture",
                "{case}"
            );
            // Remove only our junction, never recurse through it during test cleanup.
            std::fs::remove_dir(linked).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn linked_files_and_directories_are_not_cleaned() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(root.path());
        let directory = cache.library_dir("lib");
        std::fs::create_dir_all(&directory).unwrap();
        let external = outside.path().join("library-r1-1.db.download");
        old_file(&external);
        symlink(&external, directory.join("library-r1-1.db.download")).unwrap();
        symlink(outside.path(), cache.library_dir("linked")).unwrap();
        assert_eq!(cache.cleanup_stale_temporaries(), 0);
        cache.prune("linked", &[]);
        assert!(external.exists());
        assert!(directory.join("library-r1-1.db.download").is_symlink());
    }
}
