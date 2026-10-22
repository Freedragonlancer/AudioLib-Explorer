// server/audiolib-server/src/snapshot_registry.rs
//! Bounded, process-local retention of immutable snapshot representations.
//! Only the transmitted file is retained; a zstd lease never owns its source database.
use crate::state::{SnapshotEncoding, SnapshotFile};
use audiolib_core::{Error, Result};
use std::{
    io,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

const MAX_ENTRIES: usize = 4;
const MAX_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const IDLE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Identity and owned file of one immutable wire representation.
pub(crate) struct SnapshotRepresentation {
    pub file: Arc<SnapshotFile>,
    pub library_id: String,
    pub revision: u64,
    pub snapshot_format: audiolib_core::compatibility::SnapshotFormat,
    pub encoding: SnapshotEncoding,
    pub uncompressed_bytes: u64,
}

struct Entry {
    representation: SnapshotRepresentation,
    last_used: Mutex<Instant>,
}

/// A response's lease; only registry entries and leases hold an `Arc<Entry>`.
/// This makes its strong count an activity count independent of prepared-file owners.
pub(crate) struct SnapshotLease(Arc<Entry>);

impl SnapshotLease {
    pub fn representation(&self) -> &SnapshotRepresentation {
        &self.0.representation
    }
}

impl Drop for SnapshotLease {
    fn drop(&mut self) {
        *self
            .0
            .last_used
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }
}

struct RegistryInner {
    entries: Vec<Arc<Entry>>,
    closed: bool,
}

/// Completed payload budget; temporary backup/compression files are separate.
pub(crate) struct SnapshotRegistry {
    inner: Mutex<RegistryInner>,
    max_entries: usize,
    max_bytes: u64,
    ttl: Duration,
}

impl Default for SnapshotRegistry {
    fn default() -> Self {
        Self {
            inner: Mutex::new(RegistryInner {
                entries: Vec::new(),
                closed: false,
            }),
            max_entries: MAX_ENTRIES,
            max_bytes: MAX_BYTES,
            ttl: IDLE_TTL,
        }
    }
}

impl SnapshotRegistry {
    pub fn admit(&self, representation: SnapshotRepresentation) -> Result<SnapshotLease> {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if inner.closed {
            return Err(Error::Cancelled);
        }
        Self::prune_inner(&mut inner, now, self.ttl);
        if let Some(entry) = inner
            .entries
            .iter()
            .find(|entry| entry.representation.file.id == representation.file.id)
        {
            *entry
                .last_used
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = now;
            return Ok(SnapshotLease(entry.clone()));
        }
        let bytes = representation.file.bytes;
        if bytes > self.max_bytes || self.max_entries == 0 {
            return Err(capacity_error());
        }
        let mut retained = inner.entries.iter().fold(0_u64, |sum, entry| {
            sum.saturating_add(entry.representation.file.bytes)
        });
        let mut count = inner.entries.len();
        let mut candidates: Vec<_> = inner
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| Arc::strong_count(entry) == 1)
            .map(|(index, entry)| {
                (
                    index,
                    *entry
                        .last_used
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner),
                )
            })
            .collect();
        candidates.sort_by_key(|(_, last_used)| *last_used);
        let mut evict = Vec::new();
        for (index, _) in candidates {
            if count < self.max_entries && retained <= self.max_bytes - bytes {
                break;
            }
            retained -= inner.entries[index].representation.file.bytes;
            count -= 1;
            evict.push(index);
        }
        if count >= self.max_entries || retained > self.max_bytes - bytes {
            return Err(capacity_error());
        }
        // Decide all evictions first: a refusal must not erase useful idle objects.
        let mut index = 0;
        inner.entries.retain(|_| {
            let keep = !evict.contains(&index);
            index += 1;
            keep
        });
        let entry = Arc::new(Entry {
            representation,
            last_used: Mutex::new(now),
        });
        inner.entries.push(entry.clone());
        Ok(SnapshotLease(entry))
    }

    pub fn get(&self, id: &str, library_id: &str) -> Option<SnapshotLease> {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if inner.closed {
            return None;
        }
        Self::prune_inner(&mut inner, now, self.ttl);
        let entry = inner.entries.iter().find(|entry| {
            entry.representation.file.id == id && entry.representation.library_id == library_id
        })?;
        *entry
            .last_used
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = now;
        Some(SnapshotLease(entry.clone()))
    }

    /// Release the least recently used retained entry without an active lease, so a
    /// preparation short of disk space can reclaim it. Entries of `library_id` at
    /// `keep_revision` (the revision being prepared) are never released: they cannot
    /// make room for that revision and may be its resumable copy. Returns the bytes
    /// actually freed on disk: 0 when nothing idle is eligible, or when the released
    /// file is still owned elsewhere (for example by the prepared-snapshot cache).
    pub fn evict_oldest_idle(&self, library_id: &str, keep_revision: Option<u64>) -> u64 {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let oldest = inner
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| Arc::strong_count(entry) == 1)
            .filter(|(_, entry)| {
                let r = &entry.representation;
                !(r.library_id == library_id && Some(r.revision) == keep_revision)
            })
            .min_by_key(|(_, entry)| {
                *entry
                    .last_used
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
            })
            .map(|(index, _)| index);
        let Some(index) = oldest else {
            return 0;
        };
        let entry = inner.entries.remove(index);
        drop(inner);
        match Arc::try_unwrap(entry) {
            Ok(entry) => {
                let file = entry.representation.file;
                let bytes = file.bytes;
                // Only the last owner deletes the payload directory when dropped.
                if Arc::strong_count(&file) == 1 {
                    bytes
                } else {
                    0
                }
            }
            Err(_) => 0,
        }
    }

    pub fn prune(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        Self::prune_inner(&mut inner, Instant::now(), self.ttl);
    }

    pub fn close(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.closed = true;
        inner.entries.clear();
    }

    fn prune_inner(inner: &mut RegistryInner, now: Instant, ttl: Duration) {
        inner.entries.retain(|entry| {
            Arc::strong_count(entry) > 1
                || now.saturating_duration_since(
                    *entry
                        .last_used
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner),
                ) < ttl
        });
    }
}

fn capacity_error() -> Error {
    Error::Io(io::Error::new(
        io::ErrorKind::StorageFull,
        "Snapshot retention cannot admit this representation within its capacity budget",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(entries: usize, bytes: u64) -> SnapshotRegistry {
        SnapshotRegistry {
            max_entries: entries,
            max_bytes: bytes,
            ..Default::default()
        }
    }

    fn representation(bytes: u64) -> SnapshotRepresentation {
        SnapshotRepresentation {
            file: Arc::new(SnapshotFile::test_file(bytes)),
            library_id: "library".into(),
            revision: 1,
            snapshot_format: audiolib_core::compatibility::SnapshotFormat {
                id: audiolib_core::compatibility::PLAN_B_FORMAT.into(),
                format: audiolib_core::compatibility::LibraryFormat::target(),
            },
            encoding: SnapshotEncoding::Identity,
            uncompressed_bytes: bytes,
        }
    }

    fn age(registry: &SnapshotRegistry, id: &str, duration: Duration) {
        let inner = registry.inner.lock().unwrap();
        let entry = inner
            .entries
            .iter()
            .find(|entry| entry.representation.file.id == id)
            .unwrap();
        *entry.last_used.lock().unwrap() = Instant::now()
            .checked_sub(duration)
            .unwrap_or_else(Instant::now);
    }

    fn prune_after_ttl(registry: &SnapshotRegistry) {
        let mut inner = registry.inner.lock().unwrap();
        SnapshotRegistry::prune_inner(&mut inner, Instant::now() + IDLE_TTL, IDLE_TTL);
    }

    #[test]
    fn evict_oldest_idle_releases_lru_idle_entries_only() {
        let registry = registry(4, 100);
        let old = registry.admit(representation(1)).unwrap();
        let old_id = old.representation().file.id.clone();
        drop(old);
        age(&registry, &old_id, Duration::from_secs(60));
        let newer = registry.admit(representation(1)).unwrap();
        let newer_id = newer.representation().file.id.clone();
        drop(newer);
        let live = registry.admit(representation(1)).unwrap();
        assert_eq!(registry.evict_oldest_idle("library", None), 1);
        assert!(registry.get(&old_id, "library").is_none());
        assert!(registry.get(&newer_id, "library").is_some());
        assert_eq!(registry.evict_oldest_idle("library", None), 1);
        assert_eq!(registry.evict_oldest_idle("library", None), 0);
        assert!(
            registry
                .get(&live.representation().file.id, "library")
                .is_some()
        );
    }

    #[test]
    fn evict_oldest_idle_keeps_current_revision_and_reports_unfreed_files() {
        let registry = registry(4, 100);
        let mut current = representation(5);
        current.revision = 2;
        let current = registry.admit(current).unwrap();
        let current_id = current.representation().file.id.clone();
        drop(current);
        age(&registry, &current_id, Duration::from_secs(60));
        // The only idle entry is of the revision being prepared: nothing is freed.
        assert_eq!(registry.evict_oldest_idle("library", Some(2)), 0);
        assert!(registry.get(&current_id, "library").is_some());
        // An older revision whose file is still owned elsewhere frees no space.
        let shared = representation(3);
        let file = shared.file.clone();
        drop(registry.admit(shared).unwrap());
        age(&registry, &file.id, Duration::from_secs(120));
        assert_eq!(registry.evict_oldest_idle("library", Some(2)), 0);
        assert!(registry.get(&current_id, "library").is_some());
        drop(file);
        // Another library's entry at the same revision number is eligible.
        let mut other = representation(4);
        other.revision = 2;
        other.library_id = "other".into();
        drop(registry.admit(other).unwrap());
        assert_eq!(registry.evict_oldest_idle("library", Some(2)), 4);
        assert!(registry.get(&current_id, "library").is_some());
    }

    #[test]
    fn duplicate_registration_is_one_payload_and_live_leases_block_eviction() {
        let registry = registry(1, 10);
        let selected = representation(10);
        let file = selected.file.clone();
        let first = registry.admit(selected).unwrap();
        let second = registry
            .admit(SnapshotRepresentation {
                file,
                ..representation(0)
            })
            .unwrap();
        assert_eq!(registry.inner.lock().unwrap().entries.len(), 1);
        assert!(
            matches!(registry.admit(representation(1)), Err(Error::Io(error))
            if error.kind() == io::ErrorKind::StorageFull)
        );
        drop(first);
        assert!(registry.admit(representation(1)).is_err());
        drop(second);
        assert!(registry.admit(representation(1)).is_ok());
    }

    #[test]
    fn byte_budget_evicts_only_idle_lru_objects_and_failed_admission_preserves_them() {
        let registry = registry(4, 10);
        let idle = registry.admit(representation(3)).unwrap();
        let idle_id = idle.representation().file.id.clone();
        drop(idle);
        let live = registry.admit(representation(7)).unwrap();
        assert!(registry.admit(representation(4)).is_err());
        assert!(registry.get(&idle_id, "library").is_some());
        let new = registry.admit(representation(2)).unwrap();
        assert!(registry.get(&idle_id, "library").is_none());
        assert!(registry.admit(representation(11)).is_err());
        assert!(
            registry
                .get(&live.representation().file.id, "library")
                .is_some()
        );
        drop(new);
        drop(live);
    }

    #[test]
    fn count_limit_uses_lru_even_when_prepared_files_still_have_owners() {
        let registry = registry(2, 100);
        let selected = representation(1);
        let source_owner = selected.file.clone();
        let first = registry.admit(selected).unwrap();
        let first_id = first.representation().file.id.clone();
        drop(first);
        age(&registry, &first_id, Duration::from_secs(10));
        let second = registry.admit(representation(1)).unwrap();
        let second_id = second.representation().file.id.clone();
        drop(second);
        let third = registry.admit(representation(1)).unwrap();
        assert!(registry.get(&first_id, "library").is_none());
        assert!(source_owner.path.exists());
        assert!(registry.get(&second_id, "library").is_some());
        drop(third);
    }

    #[test]
    fn ttl_protects_active_transfers_and_resets_on_disconnect() {
        let registry = registry(4, 100);
        let active = registry.admit(representation(1)).unwrap();
        let active_id = active.representation().file.id.clone();
        let idle = registry.admit(representation(1)).unwrap();
        let idle_id = idle.representation().file.id.clone();
        drop(idle);
        prune_after_ttl(&registry);
        assert!(registry.get(&idle_id, "library").is_none());
        assert!(registry.get(&active_id, "library").is_some());
        drop(active);
        registry.prune();
        assert!(registry.get(&active_id, "library").is_some());
        prune_after_ttl(&registry);
        assert!(registry.get(&active_id, "library").is_none());
    }

    #[test]
    fn identity_is_library_bound_and_close_preserves_only_live_response_files() {
        let registry = registry(4, 100);
        let active = registry.admit(representation(1)).unwrap();
        let id = active.representation().file.id.clone();
        let path = active.representation().file.path.clone();
        assert!(registry.get(&id, "other-library").is_none());
        registry.close();
        assert!(registry.get(&id, "library").is_none());
        assert!(matches!(
            registry.admit(representation(1)),
            Err(Error::Cancelled)
        ));
        assert!(path.exists());
        drop(active);
        assert!(!path.exists());
    }
}
