// crates/audiolib-db/src/local_relocation.rs
//! Publish an independent SQLite copy while excluding other writers through desktop handoff.
use super::*;

/// Keep the source quiescent and the destination reserved until preferences and provider are switched.
pub struct LocalRelocationGuard {
    _source_scan: management::LibraryLease,
    _source_admin: management::AdministrationLease,
    _destination: import_lease::ImportDestinationLease,
}

impl SqliteProvider {
    /// Initialize a new user-selected library before publishing it without replacement.
    pub fn create_new_local(
        path: &Path,
        data_root: &Path,
        limit: u32,
        cancel: &AtomicBool,
        mut progress: impl FnMut(SnapshotUpgradeProgress),
    ) -> Result<Self> {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let lease = import_lease::ImportDestinationLease::acquire(path)?;
        let destination = lease.destination();
        management::check_new_database_sidecars(destination)?;
        let parent = destination
            .parent()
            .ok_or_else(|| Error::Validation("New library needs a parent directory".into()))?;
        // SQLite journals and persistent lock files created during initialization
        // stay inside this owned directory even when initialization is interrupted.
        let owned = tempfile::Builder::new()
            .prefix("audiolib-create-")
            .tempdir_in(parent)?;
        let mut builder = tempfile::Builder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Preserve the mode that a regular database creation gets from the umask.
            builder.permissions(fs::Permissions::from_mode(0o666));
        }
        let staged = builder.suffix(".db").tempfile_in(owned.path())?;
        progress(SnapshotUpgradeProgress::Preparing);
        migration::prepare_local_writable(staged.path(), data_root, limit, cancel, &mut progress)?;
        management::check_new_database_sidecars(staged.path())?;
        staged.as_file().sync_all()?;

        let _admin = management::AdministrationLease::shared_for_creation(&lease)?;
        management::check_new_database_sidecars(destination)?;
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        // Publication commits the completed database, after all cancellable work.
        staged
            .persist_noclobber(destination)
            .map_err(|error| Error::Io(error.error))?;
        Self::open_prepared_writable(destination)
    }
    /// Copy the active local database and its path-scoped personal data; keep the source usable on any failure.
    pub fn prepare_local_relocation(
        &self,
        path: &Path,
        data_root: &Path,
        limit: u32,
        cancel: &AtomicBool,
    ) -> Result<(Self, LocalRelocationGuard)> {
        self.require_writable()?;
        let source_scan = management::LibraryLease::acquire(&self.path)?;
        let source_admin = management::AdministrationLease::exclusive(&self.path)?;
        self.check_vendor_relocation_destination(path)?;
        let guard = LocalRelocationGuard {
            _source_scan: source_scan,
            _source_admin: source_admin,
            _destination: import_lease::ImportDestinationLease::acquire(path)?,
        };
        let destination = guard._destination.destination();
        self.backup_library_cancellable(destination, cancel)?;
        let prepared = (|| -> Result<Self> {
            if !Self::check_owned_file_cancellable(destination, cancel)?.is_ok() {
                return Err(Error::Validation(
                    "Database copy failed integrity checks".into(),
                ));
            }
            let provider = Self::open_with_history_owner_cancellable(
                destination,
                Some((data_root, limit)),
                cancel,
                |_| {},
                Some(&guard._destination),
            )?;
            self.copy_vendor_staging(destination)?;
            let old = local_play_history::local_library_id(&self.path)?;
            let new = local_play_history::local_library_id(destination)?;
            search_workspace::copy_scope(data_root, &old, &new)?;
            local_play_history::LocalPlayHistory::copy_scope(data_root, &old, &new)?;
            Ok(provider)
        })()
        .map_err(|e| {
            Error::Validation(format!("Database copy: {} — {e}", destination.display()))
        })?;
        Ok((prepared, guard))
    }
}
