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
    /// Create a new user-selected library without ever adopting an existing file.
    pub fn create_new_local(
        path: &Path,
        data_root: &Path,
        limit: u32,
        cancel: &AtomicBool,
        progress: impl FnMut(SnapshotUpgradeProgress),
    ) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let lease = import_lease::ImportDestinationLease::acquire(path)?;
        management::check_new_database_sidecars(lease.destination())?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lease.destination())?;
        Self::open_with_history_owner_cancellable(
            lease.destination(),
            Some((data_root, limit)),
            cancel,
            progress,
            Some(&lease),
        )
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
        let guard = LocalRelocationGuard {
            _source_scan: management::LibraryLease::acquire(&self.path)?,
            _source_admin: management::AdministrationLease::exclusive(&self.path)?,
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
