// server/audiolib-server/src/revision.rs
//! Stable library identity and a monotonic change revision.
//!
//! Library schema v3 has no identity column, so both values live in a server-owned
//! sidecar `<database>.server-state.json` next to the library, written atomically.
//! A dedicated read-only SQLite connection polls `PRAGMA data_version`, which changes
//! whenever another connection or process commits (including commits that so far only
//! reached the WAL); the main database file's size and mtime additionally catch changes
//! made while the server was stopped (a writer that closes checkpoints into the main
//! file). WAL size/mtime are deliberately not part of that fingerprint: they churn on
//! every open/close, which used to bump the revision on each restart. Commits that only
//! reached the WAL while the server was stopped are caught at startup by a WAL signature
//! (file length plus its 32-byte header with checkpoint sequence and salts, no mtime),
//! which a plain open does not change. Graceful shutdown checkpoints under the library
//! administration lease and persists that new physical baseline before closing.
//!
//! The server's own writes go through [`RevisionTracker::record_own_write`], which holds
//! the tracker lock across the write so an isolated change bumps the revision exactly
//! once and the poller does not count it again. A writer-connection observation detects
//! concurrent local edits and invalidates incremental replies that omit those edits.
//! A remote scan (`crate::scans`) commits many small
//! transactions; it holds the revision ([`RevisionTracker::begin_hold`]) so polling does
//! not bump on every batch, and [`RevisionTracker::end_hold`] bumps once at the end (or
//! not at all for a scan that changed nothing while nobody else wrote). The revision is a conservative change hint: it may
//! advance without a logical change (for example a WAL checkpoint) but a committed change
//! is never left without a later bump.
use audiolib_core::{Error, Result};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::{Duration, UNIX_EPOCH},
};

const SIDECAR_SUFFIX: &str = ".server-state.json";
const SIDECAR_FORMAT: u32 = 1;

/// Identity and current revision of the served library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionInfo {
    /// Stable identifier generated once per served library database path.
    pub library_id: String,
    /// Monotonic change counter, persisted across restarts.
    pub revision: u64,
}

/// Main database file size and mtime. Older sidecars also stored WAL fields; serde
/// ignores them, so upgrading does not bump the revision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct FileFingerprint {
    database_bytes: u64,
    database_mtime_ns: u64,
}

impl FileFingerprint {
    fn read(database: &Path) -> Result<Self> {
        let database_meta = fs::metadata(database)?;
        Ok(Self {
            database_bytes: database_meta.len(),
            database_mtime_ns: mtime_ns(&database_meta),
        })
    }
}

/// Result of a server write recorded by [`RevisionTracker::record_own_write`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteOutcome<T> {
    /// Value returned by the write.
    pub value: T,
    /// Revision covering every commit before this write.
    pub revision_before: u64,
    /// Revision after this write (`revision_before + 1`, or `revision_before` for a
    /// write that changed no snapshot content).
    pub revision: u64,
}

pub(crate) fn mtime_ns(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SidecarState {
    format: u32,
    library_id: String,
    revision: u64,
    fingerprint: FileFingerprint,
    /// WAL signature at the last bump; absent in older sidecars (then not compared).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wal: Option<WalSignature>,
}

/// Length and header of the `-wal` file, compared only at startup. The header changes
/// whenever SQLite restarts the WAL (new salts), and appended commits grow the file, so a
/// WAL-only commit made while the server was stopped always changes the signature.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct WalSignature {
    bytes: u64,
    /// Hex of the first 32 bytes (empty for a missing or empty WAL).
    header: String,
}

impl WalSignature {
    fn read(database: &Path) -> Result<Self> {
        use std::io::Read;
        let mut path = database.as_os_str().to_os_string();
        path.push("-wal");
        let file = match fs::File::open(PathBuf::from(path)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.into()),
        };
        let bytes = file.metadata()?.len();
        let mut header = Vec::with_capacity(32);
        file.take(32).read_to_end(&mut header)?;
        let header = header.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Self { bytes, header })
    }
}

/// Path of the server-owned state sidecar for a library database.
pub fn sidecar_path(database: &Path) -> PathBuf {
    let mut name = database.file_name().unwrap_or_default().to_os_string();
    name.push(SIDECAR_SUFFIX);
    database.with_file_name(name)
}

fn db_error(error: rusqlite::Error) -> Error {
    Error::Database(error.to_string())
}

fn data_version(conn: &Connection) -> Result<i64> {
    conn.pragma_query_value(None, "data_version", |row| row.get(0))
        .map_err(db_error)
}

fn read_sidecar(path: &Path) -> Result<Option<SidecarState>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    // Never silently regenerate the identity: clients would discard their cached state.
    let state: SidecarState = serde_json::from_str(&text).map_err(|error| {
        Error::Validation(format!(
            "Server state {} is unreadable ({error}); restore it or delete it to issue a new library_id",
            path.display()
        ))
    })?;
    // The id is sent verbatim in `X-Library-Id`, so it must be a valid header value.
    let id = state.library_id.as_str();
    if id.is_empty() || id.len() > 128 || !id.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Error::Validation(format!(
            "Server state {} has an invalid library_id (1..=128 visible ASCII characters required)",
            path.display()
        )));
    }
    if state.format != SIDECAR_FORMAT {
        return Err(Error::Validation(format!(
            "Server state {} has unsupported format {}",
            path.display(),
            state.format
        )));
    }
    Ok(Some(state))
}

fn write_sidecar(path: &Path, state: &SidecarState) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut stage = tempfile::Builder::new()
        .prefix(".audiolib-server-state-")
        .tempfile_in(parent)?;
    let json = serde_json::to_vec_pretty(state)
        .map_err(|error| Error::Validation(format!("Cannot encode server state: {error}")))?;
    stage.write_all(&json)?;
    stage.as_file().sync_all()?;
    stage
        .persist(path)
        .map_err(|error| Error::Io(error.error))?;
    Ok(())
}

struct TrackerInner {
    conn: Connection,
    data_version: i64,
    state: SidecarState,
    /// Active holds ([`RevisionTracker::begin_hold`]); polling does not bump while > 0.
    holds: usize,
    /// A write attempted during a hold returned before its complete baseline was
    /// recorded. Even an otherwise unchanged scan cannot absorb that possible commit.
    uncertain_write: bool,
}

/// Owns the dedicated change-detection connection and the persisted identity/revision.
pub struct RevisionTracker {
    database: PathBuf,
    sidecar: PathBuf,
    inner: Mutex<TrackerInner>,
}

impl RevisionTracker {
    /// Drain outstanding revisions, checkpoint, and persist the physical baseline
    /// while library writers are excluded by the administration lease. Direct SQLite
    /// writers must follow that same lease protocol, just as for restore/maintenance.
    pub fn checkpoint(&self, provider: &audiolib_db::SqliteProvider) -> Result<()> {
        let mut inner = self.lock()?;
        if inner.holds != 0 {
            return Err(Error::Busy("A library write is still finishing".into()));
        }
        provider.checkpoint_wal_observed(|stage| {
            if stage == audiolib_db::CheckpointStage::Before {
                self.catch_up(&mut inner)?;
            } else {
                let version = data_version(&inner.conn)?;
                let fingerprint = FileFingerprint::read(&self.database)?;
                // SQLite can change this connection's data_version when TRUNCATE
                // resets the WAL. All admitted commits were counted in Before and
                // the exclusive administration lease still excludes library writers.
                let mut next = inner.state.clone();
                next.fingerprint = fingerprint;
                next.wal = Some(WalSignature::read(&self.database)?);
                write_sidecar(&self.sidecar, &next)?;
                inner.state = next;
                inner.data_version = version;
            }
            Ok(())
        })
    }
    /// Blocking. Open the change-detection connection and load or create the sidecar.
    /// A database changed since the last recorded fingerprint bumps the revision once.
    pub fn open(database: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            database,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(db_error)?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(db_error)?;
        conn.execute_batch("PRAGMA query_only=ON;")
            .map_err(db_error)?;
        let data_version = data_version(&conn)?;
        let fingerprint = FileFingerprint::read(database)?;
        let wal = WalSignature::read(database)?;
        let sidecar = sidecar_path(database);
        let state = match read_sidecar(&sidecar)? {
            Some(mut state) => {
                let wal_changed = state.wal.as_ref().is_some_and(|stored| *stored != wal);
                if state.fingerprint != fingerprint || wal_changed {
                    state.revision = next_revision(state.revision)?;
                    state.fingerprint = fingerprint;
                    state.wal = Some(wal);
                    write_sidecar(&sidecar, &state)?;
                    tracing::info!(
                        revision = state.revision,
                        "Library changed while the server was stopped"
                    );
                }
                state
            }
            None => {
                let state = SidecarState {
                    format: SIDECAR_FORMAT,
                    library_id: uuid::Uuid::new_v4().to_string(),
                    revision: 1,
                    fingerprint,
                    wal: Some(wal),
                };
                write_sidecar(&sidecar, &state)?;
                tracing::info!(
                    library_id = %state.library_id,
                    sidecar = %sidecar.display(),
                    "Created library identity"
                );
                state
            }
        };
        Ok(Self {
            database: database.to_path_buf(),
            sidecar,
            inner: Mutex::new(TrackerInner {
                conn,
                data_version,
                state,
                holds: 0,
                uncertain_write: false,
            }),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, TrackerInner>> {
        self.inner
            .lock()
            .map_err(|_| Error::Database("Revision tracker lock was poisoned".into()))
    }

    /// Last known identity and revision without touching the database.
    pub fn current(&self) -> Result<RevisionInfo> {
        Ok(info(&self.lock()?.state))
    }

    /// Blocking. Check for committed changes and bump the revision when one is seen.
    /// While a hold is active (a running scan) the current revision is returned as is;
    /// the changes are counted by [`RevisionTracker::end_hold`] (or the next server write).
    pub fn poll(&self) -> Result<RevisionInfo> {
        let mut inner = self.lock()?;
        if inner.holds == 0 {
            self.catch_up(&mut inner)?;
        }
        Ok(info(&inner.state))
    }

    /// Start holding the revision: polls stop counting commits until the matching
    /// [`RevisionTracker::end_hold`]. Server writes ([`RevisionTracker::record_own_write`])
    /// still bump (covering everything committed so far). Snapshots taken meanwhile are
    /// labelled with the held revision and are at least that new.
    pub fn begin_hold(&self) -> Result<()> {
        self.lock()?.holds += 1;
        Ok(())
    }

    /// Blocking. End a hold. With `changed` the revision is bumped once (always, even
    /// if a server write already counted part of the work). Without it, commits made
    /// during the hold become the new baseline without a bump: the caller asserts they
    /// carry no client-visible change and that no other connection wrote meanwhile.
    /// A write that returned before recording its baseline overrides that assertion:
    /// its possible commit is conservatively counted even when `changed` is false.
    pub fn end_hold(&self, changed: bool) -> Result<RevisionInfo> {
        let mut inner = self.lock()?;
        inner.holds = inner.holds.saturating_sub(1);
        let version = data_version(&inner.conn)?;
        let fingerprint = FileFingerprint::read(&self.database)?;
        if changed || inner.uncertain_write {
            self.advance(&mut inner, version, fingerprint)?;
            tracing::info!(revision = inner.state.revision, "Library changed by a scan");
        } else if version != inner.data_version || fingerprint != inner.state.fingerprint {
            // Same revision, new baseline (also persisted, so a restart does not bump).
            let mut next = inner.state.clone();
            next.fingerprint = fingerprint;
            next.wal = Some(WalSignature::read(&self.database)?);
            write_sidecar(&self.sidecar, &next)?;
            inner.state = next;
            inner.data_version = version;
        }
        Ok(info(&inner.state))
    }

    /// Bump once if anything changed since the last baseline.
    fn catch_up(&self, inner: &mut TrackerInner) -> Result<()> {
        let version = data_version(&inner.conn)?;
        let fingerprint = FileFingerprint::read(&self.database)?;
        if inner.uncertain_write
            || version != inner.data_version
            || fingerprint != inner.state.fingerprint
        {
            self.advance(inner, version, fingerprint)?;
            tracing::info!(revision = inner.state.revision, "Library change detected");
        }
        Ok(())
    }

    /// Blocking. Run a server-side write so that it bumps the revision exactly once.
    ///
    /// Under the tracker lock (which also serializes server writes and excludes the
    /// poller): pending external changes are first counted (so `revision_before` covers
    /// every earlier commit), then `write` runs, then the revision becomes
    /// `revision_before + 1` and the post-write `data_version`/fingerprint become the new
    /// baseline, so the poller does not bump again for this write. A failed write leaves
    /// the revision untouched (the next poll still catches any partial effect).
    ///
    /// `foreign_version` must read `PRAGMA data_version` on the connection performing
    /// `write`: its own commits leave that value unchanged, whereas commits from a
    /// local client or another connection change it. Observations bracket both the
    /// tracker baseline and the operation. A concurrent foreign commit advances
    /// `revision_before` first, so a client cannot apply only the returned records to
    /// an older snapshot and incorrectly declare itself current.
    ///
    /// During a hold (a running scan) pending commits are only counted when the write
    /// succeeds (then `revision_before` = held + 1 and `revision` = held + 2), so a
    /// rejected write (for example 409 `BUSY` batch edits) leaves the held revision alone.
    pub fn record_own_write<T>(
        &self,
        write: impl FnOnce() -> Result<T>,
        foreign_version: impl FnMut() -> Result<i64>,
    ) -> Result<WriteOutcome<T>> {
        self.record_own_write_if(write, |_| true, foreign_version)
    }

    /// Blocking. [`RevisionTracker::record_own_write`] for a write that may turn out to
    /// change nothing (removing records that no longer exist): when `changed` says so for
    /// the returned value, the revision is not bumped and `revision == revision_before`.
    /// The baseline is then left alone, so a commit the write made anyway is still
    /// counted by the next poll. A concurrent foreign commit is counted immediately,
    /// including for a no-op, so it cannot be hidden by the returned revision.
    /// `foreign_version` has the same writer-connection contract as
    /// [`Self::record_own_write`].
    pub fn record_own_write_if<T>(
        &self,
        write: impl FnOnce() -> Result<T>,
        changed: impl FnOnce(&T) -> bool,
        mut foreign_version: impl FnMut() -> Result<i64>,
    ) -> Result<WriteOutcome<T>> {
        let mut inner = self.lock()?;
        // Observe before catch_up: a commit between catch_up and this observation
        // would otherwise be absorbed into the successful write's final baseline.
        let foreign_before = foreign_version()?;
        let deferred = if inner.holds == 0 {
            self.catch_up(&mut inner)?;
            false
        } else {
            data_version(&inner.conn)? != inner.data_version
                || FileFingerprint::read(&self.database)? != inner.state.fingerprint
        };
        // Once the operation is attempted, every error is potentially post-commit.
        // Keep that uncertainty across an otherwise unchanged scan's end_hold(false).
        let result = (|| {
            let value = write()?;
            let changed = changed(&value);
            let version = data_version(&inner.conn)?;
            let fingerprint = FileFingerprint::read(&self.database)?;
            // Observe after the tracker baseline. A later foreign commit remains visible
            // to poll via `version`; an earlier one must invalidate incremental replies.
            let foreign_changed = foreign_version()? != foreign_before;
            if !changed {
                if foreign_changed || inner.uncertain_write {
                    self.advance(&mut inner, version, fingerprint)?;
                }
                let revision = inner.state.revision;
                return Ok(WriteOutcome {
                    value,
                    revision_before: revision,
                    revision,
                });
            }
            if deferred || foreign_changed || inner.uncertain_write {
                self.advance(&mut inner, version, fingerprint)?;
            }
            let revision_before = inner.state.revision;
            self.advance(&mut inner, version, fingerprint)?;
            Ok(WriteOutcome {
                value,
                revision_before,
                revision: inner.state.revision,
            })
        })();
        if result.is_err() && inner.holds > 0 {
            inner.uncertain_write = true;
        }
        result
    }

    /// Blocking. Run a server-side write that only touches data clients do not need to
    /// re-download a snapshot for. Under the tracker lock, pending commits from other writers are
    /// counted first (so `revision_before` covers them), then `write` runs and its commit
    /// becomes the new polling baseline (persisted, so a restart does not bump either)
    /// without advancing the revision: `revision == revision_before`. During a hold the
    /// baseline is left alone so the scan's commits are still counted by `end_hold`; the
    /// caller's commit then rides along with that bump. Concurrent foreign commits
    /// advance the revision even for a quiet write. `foreign_version` must observe
    /// the writer connection, as required by [`Self::record_own_write`].
    pub fn record_own_write_quiet<T>(
        &self,
        write: impl FnOnce() -> Result<T>,
        mut foreign_version: impl FnMut() -> Result<i64>,
    ) -> Result<WriteOutcome<T>> {
        let mut inner = self.lock()?;
        let foreign_before = foreign_version()?;
        if inner.holds == 0 {
            self.catch_up(&mut inner)?;
        }
        let result = (|| {
            let value = write()?;
            let version = data_version(&inner.conn)?;
            let fingerprint = FileFingerprint::read(&self.database)?;
            if foreign_version()? != foreign_before || inner.uncertain_write {
                self.advance(&mut inner, version, fingerprint)?;
            } else if inner.holds == 0
                && (version != inner.data_version || fingerprint != inner.state.fingerprint)
            {
                let mut next = inner.state.clone();
                next.fingerprint = fingerprint;
                next.wal = Some(WalSignature::read(&self.database)?);
                write_sidecar(&self.sidecar, &next)?;
                inner.state = next;
                inner.data_version = version;
            }
            let revision = inner.state.revision;
            Ok(WriteOutcome {
                value,
                revision_before: revision,
                revision,
            })
        })();
        if result.is_err() && inner.holds > 0 {
            inner.uncertain_write = true;
        }
        result
    }

    /// Blocking. Unconditionally bump the revision; call after server-side writes.
    pub fn bump(&self) -> Result<RevisionInfo> {
        let mut inner = self.lock()?;
        let version = data_version(&inner.conn)?;
        let fingerprint = FileFingerprint::read(&self.database)?;
        self.advance(&mut inner, version, fingerprint)?;
        tracing::debug!(
            revision = inner.state.revision,
            "Revision bumped by server write"
        );
        Ok(info(&inner.state))
    }

    // Persist first so the in-memory revision never runs ahead of the sidecar.
    fn advance(
        &self,
        inner: &mut TrackerInner,
        version: i64,
        fingerprint: FileFingerprint,
    ) -> Result<()> {
        let mut next = inner.state.clone();
        next.revision = next_revision(next.revision)?;
        next.fingerprint = fingerprint;
        next.wal = Some(WalSignature::read(&self.database)?);
        write_sidecar(&self.sidecar, &next)?;
        inner.state = next;
        inner.data_version = version;
        inner.uncertain_write = false;
        Ok(())
    }
}

fn next_revision(revision: u64) -> Result<u64> {
    revision
        .checked_add(1)
        .ok_or_else(|| Error::Database("Library revision overflow".into()))
}

fn info(state: &SidecarState) -> RevisionInfo {
    RevisionInfo {
        library_id: state.library_id.clone(),
        revision: state.revision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database(dir: &Path) -> (PathBuf, Connection) {
        let path = dir.join("lib.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(x);")
            .unwrap();
        (path, conn)
    }

    #[test]
    fn identity_persists_and_commits_bump_revision_monotonically() {
        let dir = tempfile::tempdir().unwrap();
        let (path, writer) = database(dir.path());
        let tracker = RevisionTracker::open(&path).unwrap();
        let first = tracker.poll().unwrap();
        assert_eq!(tracker.poll().unwrap(), first, "no change, no bump");
        writer.execute("INSERT INTO t VALUES (1)", []).unwrap();
        let second = tracker.poll().unwrap();
        assert_eq!(second.library_id, first.library_id);
        assert!(second.revision > first.revision);
        let bumped = tracker.bump().unwrap();
        assert_eq!(bumped.revision, second.revision + 1);
        drop(tracker);
        let reopened = RevisionTracker::open(&path).unwrap().current().unwrap();
        assert_eq!(reopened.library_id, first.library_id);
        assert!(reopened.revision >= bumped.revision);
    }

    #[test]
    fn sidecar_library_id_must_be_a_valid_header_value() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _writer) = database(dir.path());
        let tracker = RevisionTracker::open(&path).unwrap();
        drop(tracker);
        let sidecar = sidecar_path(&path);
        let text = fs::read_to_string(&sidecar).unwrap();
        let mut state: serde_json::Value = serde_json::from_str(&text).unwrap();
        for bad in ["", "has space", "line\nbreak", "é"] {
            state["library_id"] = bad.into();
            fs::write(&sidecar, serde_json::to_vec(&state).unwrap()).unwrap();
            assert!(
                matches!(RevisionTracker::open(&path), Err(Error::Validation(_))),
                "{bad:?}"
            );
        }
    }

    fn wal_writer(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        // Keep commits in the WAL only, like a busy writer before its next checkpoint.
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        conn
    }

    #[test]
    fn restarts_and_plain_opens_do_not_bump() {
        let dir = tempfile::tempdir().unwrap();
        let (path, writer) = database(dir.path());
        writer.execute("INSERT INTO t VALUES (1)", []).unwrap();
        let first = RevisionTracker::open(&path).unwrap().poll().unwrap();
        // Another connection opening (and resizing the WAL) is not a change.
        let other = wal_writer(&path);
        drop(other);
        for _ in 0..3 {
            let reopened = RevisionTracker::open(&path).unwrap();
            assert_eq!(reopened.current().unwrap(), first);
            assert_eq!(reopened.poll().unwrap(), first);
        }
    }

    #[test]
    fn wal_only_commits_made_while_stopped_are_detected_at_startup() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _setup) = database(dir.path());
        let writer = wal_writer(&path);
        writer.execute("INSERT INTO t VALUES (1)", []).unwrap();
        let first = RevisionTracker::open(&path).unwrap().poll().unwrap();
        let main_before = FileFingerprint::read(&path).unwrap();
        writer.execute("INSERT INTO t VALUES (2)", []).unwrap();
        assert_eq!(FileFingerprint::read(&path).unwrap(), main_before);
        let reopened = RevisionTracker::open(&path).unwrap().current().unwrap();
        assert_eq!(reopened.revision, first.revision + 1);
        assert_eq!(
            RevisionTracker::open(&path).unwrap().current().unwrap(),
            reopened
        );
    }

    #[test]
    fn wal_only_commits_from_another_connection_are_detected() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _setup) = database(dir.path());
        let tracker = RevisionTracker::open(&path).unwrap();
        let first = tracker.poll().unwrap();
        let writer = wal_writer(&path);
        let main_before = FileFingerprint::read(&path).unwrap();
        writer.execute("INSERT INTO t VALUES (2)", []).unwrap();
        assert_eq!(
            FileFingerprint::read(&path).unwrap(),
            main_before,
            "the commit only reached the WAL"
        );
        let second = tracker.poll().unwrap();
        assert_eq!(second.revision, first.revision + 1);
        assert_eq!(tracker.poll().unwrap(), second, "counted once");
    }

    #[test]
    fn own_writes_bump_exactly_once_after_pending_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _setup) = database(dir.path());
        let tracker = RevisionTracker::open(&path).unwrap();
        let start = tracker.poll().unwrap().revision;
        let external = wal_writer(&path);
        external.execute("INSERT INTO t VALUES (3)", []).unwrap();
        let own = wal_writer(&path);
        let outcome = tracker
            .record_own_write(
                || {
                    own.execute("INSERT INTO t VALUES (4)", [])
                        .map_err(db_error)
                },
                || data_version(&own),
            )
            .unwrap();
        assert_eq!(outcome.value, 1);
        assert_eq!(
            outcome.revision_before,
            start + 1,
            "external change counted first"
        );
        assert_eq!(outcome.revision, outcome.revision_before + 1);
        assert_eq!(
            tracker.poll().unwrap().revision,
            outcome.revision,
            "no second bump"
        );
        let failed = tracker.record_own_write(
            || -> Result<()> { Err(Error::NotFound("x".into())) },
            || data_version(&own),
        );
        assert!(failed.is_err());
        assert_eq!(tracker.poll().unwrap().revision, outcome.revision);
        drop(tracker);
        let reopened = RevisionTracker::open(&path).unwrap();
        assert_eq!(reopened.current().unwrap().revision, outcome.revision);
    }

    #[test]
    fn holds_defer_polling_and_end_with_one_bump_or_none() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _setup) = database(dir.path());
        let tracker = RevisionTracker::open(&path).unwrap();
        let start = tracker.poll().unwrap().revision;
        let writer = wal_writer(&path);
        tracker.begin_hold().unwrap();
        for value in 0..3 {
            writer
                .execute("INSERT INTO t VALUES (?1)", [value])
                .unwrap();
            assert_eq!(tracker.poll().unwrap().revision, start, "held");
        }
        assert_eq!(tracker.end_hold(true).unwrap().revision, start + 1);
        assert_eq!(tracker.poll().unwrap().revision, start + 1, "counted once");
        // A no-op hold absorbs its commits without a bump, also across a restart.
        tracker.begin_hold().unwrap();
        writer.execute("INSERT INTO t VALUES (9)", []).unwrap();
        assert_eq!(tracker.end_hold(false).unwrap().revision, start + 1);
        assert_eq!(tracker.poll().unwrap().revision, start + 1);
        drop(tracker);
        let reopened = RevisionTracker::open(&path).unwrap();
        assert_eq!(reopened.current().unwrap().revision, start + 1);
        // Commits after the hold are counted again.
        writer.execute("INSERT INTO t VALUES (10)", []).unwrap();
        assert_eq!(reopened.poll().unwrap().revision, start + 2);
    }

    #[test]
    fn server_writes_during_a_hold_count_pending_commits_only_when_they_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _setup) = database(dir.path());
        let tracker = RevisionTracker::open(&path).unwrap();
        let start = tracker.poll().unwrap().revision;
        let writer = wal_writer(&path);
        tracker.begin_hold().unwrap();
        writer.execute("INSERT INTO t VALUES (1)", []).unwrap();
        let rejected = tracker.record_own_write(
            || -> Result<()> { Err(Error::Busy("scan".into())) },
            || data_version(&writer),
        );
        assert!(rejected.is_err());
        assert_eq!(
            tracker.poll().unwrap().revision,
            start,
            "a rejected write keeps the hold"
        );
        let outcome = tracker
            .record_own_write(
                || {
                    writer.execute("INSERT INTO t VALUES (2)", []).unwrap();
                    Ok(())
                },
                || data_version(&writer),
            )
            .unwrap();
        assert_eq!(
            (outcome.revision_before, outcome.revision),
            (start + 1, start + 2)
        );
        assert_eq!(tracker.end_hold(false).unwrap().revision, start + 2);
    }

    #[test]
    fn conditional_writes_bump_only_when_they_changed_something() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _setup) = database(dir.path());
        let tracker = RevisionTracker::open(&path).unwrap();
        let start = tracker.poll().unwrap().revision;
        let writer = wal_writer(&path);
        // Nothing changed: same revision on both sides, nothing persisted.
        let outcome = tracker
            .record_own_write_if(|| Ok(0_u64), |n| *n > 0, || data_version(&writer))
            .unwrap();
        assert_eq!((outcome.revision_before, outcome.revision), (start, start));
        assert_eq!(tracker.poll().unwrap().revision, start);
        // A pending external commit is counted first, even by a no-op write.
        writer.execute("INSERT INTO t VALUES (1)", []).unwrap();
        let outcome = tracker
            .record_own_write_if(|| Ok(0_u64), |n| *n > 0, || data_version(&writer))
            .unwrap();
        assert_eq!(
            (outcome.revision_before, outcome.revision),
            (start + 1, start + 1)
        );
        // A real change bumps exactly once and the poller does not count it again.
        let outcome = tracker
            .record_own_write_if(
                || {
                    writer.execute("INSERT INTO t VALUES (2)", []).unwrap();
                    Ok(1_u64)
                },
                |n| *n > 0,
                || data_version(&writer),
            )
            .unwrap();
        assert_eq!(
            (outcome.revision_before, outcome.revision),
            (start + 1, start + 2)
        );
        assert_eq!(tracker.poll().unwrap().revision, start + 2);
        // A write that reports no change but committed anyway is still caught by a poll.
        tracker
            .record_own_write_if(
                || {
                    writer.execute("INSERT INTO t VALUES (3)", []).unwrap();
                    Ok(0_u64)
                },
                |n| *n > 0,
                || data_version(&writer),
            )
            .unwrap();
        assert_eq!(tracker.poll().unwrap().revision, start + 3);
    }

    #[test]
    fn quiet_writes_leave_the_revision_alone_and_content_writes_still_bump() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _setup) = database(dir.path());
        let tracker = RevisionTracker::open(&path).unwrap();
        let start = tracker.poll().unwrap().revision;
        let writer = wal_writer(&path);
        // A non-content write commits but neither it nor a later poll bumps.
        let outcome = tracker
            .record_own_write_quiet(
                || {
                    writer.execute("INSERT INTO t VALUES (1)", []).unwrap();
                    Ok(())
                },
                || data_version(&writer),
            )
            .unwrap();
        assert_eq!((outcome.revision_before, outcome.revision), (start, start));
        assert_eq!(tracker.poll().unwrap().revision, start);
        // Nor does a restart.
        drop(tracker);
        let tracker = RevisionTracker::open(&path).unwrap();
        assert_eq!(tracker.poll().unwrap().revision, start);
        // An external commit pending before a quiet write is still counted.
        writer.execute("INSERT INTO t VALUES (2)", []).unwrap();
        let outcome = tracker
            .record_own_write_quiet(
                || {
                    writer.execute("INSERT INTO t VALUES (3)", []).unwrap();
                    Ok(())
                },
                || data_version(&writer),
            )
            .unwrap();
        assert_eq!(
            (outcome.revision_before, outcome.revision),
            (start + 1, start + 1)
        );
        assert_eq!(tracker.poll().unwrap().revision, start + 1);
        // A content write bumps exactly once.
        let outcome = tracker
            .record_own_write(
                || {
                    writer.execute("INSERT INTO t VALUES (4)", []).unwrap();
                    Ok(())
                },
                || data_version(&writer),
            )
            .unwrap();
        assert_eq!(outcome.revision, start + 2);
        assert_eq!(tracker.poll().unwrap().revision, start + 2);
        // During a hold the quiet write does not swallow the scan's commits.
        tracker.begin_hold().unwrap();
        writer.execute("INSERT INTO t VALUES (5)", []).unwrap();
        tracker
            .record_own_write_quiet(
                || {
                    writer.execute("INSERT INTO t VALUES (6)", []).unwrap();
                    Ok(())
                },
                || data_version(&writer),
            )
            .unwrap();
        assert_eq!(tracker.end_hold(true).unwrap().revision, start + 3);
    }

    #[test]
    fn foreign_commits_at_observation_boundaries_cannot_disappear_into_own_revision() {
        use std::cell::Cell;
        for boundary in [
            "before_catch_up",
            "during_write",
            "before_final_sample",
            "after_final_sample",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (path, _setup) = database(dir.path());
            let tracker = RevisionTracker::open(&path).unwrap();
            let writer = wal_writer(&path);
            let external = wal_writer(&path);
            let start = tracker.poll().unwrap().revision;
            let observations = Cell::new(0);
            let foreign_commit = || {
                // Multiple commits can coalesce into one observed data_version change;
                // correctness must not depend on subtracting version counters.
                external.execute("INSERT INTO t VALUES (10)", []).unwrap();
                external.execute("INSERT INTO t VALUES (11)", []).unwrap();
            };
            let outcome = tracker
                .record_own_write(
                    || {
                        if boundary == "during_write" {
                            foreign_commit();
                        }
                        writer
                            .execute("INSERT INTO t VALUES (1)", [])
                            .map_err(db_error)
                    },
                    || {
                        let sample = observations.get();
                        observations.set(sample + 1);
                        if sample == 1 && boundary == "before_final_sample" {
                            foreign_commit();
                        }
                        let version = data_version(&writer)?;
                        if (sample == 0 && boundary == "before_catch_up")
                            || (sample == 1 && boundary == "after_final_sample")
                        {
                            foreign_commit();
                        }
                        Ok(version)
                    },
                )
                .unwrap();
            if boundary == "after_final_sample" {
                assert_eq!(outcome.revision_before, start);
                assert!(tracker.poll().unwrap().revision > outcome.revision);
            } else {
                assert!(
                    outcome.revision_before > start,
                    "{boundary} must refuse an old incremental cache"
                );
                assert!(tracker.poll().unwrap().revision >= outcome.revision);
            }
        }
    }

    #[test]
    fn failed_noop_and_quiet_writes_preserve_concurrent_changes_during_scan_holds() {
        for holding in [false, true] {
            for kind in ["failed", "noop", "quiet", "ordinary"] {
                let dir = tempfile::tempdir().unwrap();
                let (path, _setup) = database(dir.path());
                let tracker = RevisionTracker::open(&path).unwrap();
                let writer = wal_writer(&path);
                let external = wal_writer(&path);
                let start = tracker.poll().unwrap().revision;
                if holding {
                    tracker.begin_hold().unwrap();
                }
                let write = || {
                    external.execute("INSERT INTO t VALUES (10)", []).unwrap();
                    if kind == "failed" {
                        return Err(Error::Busy("scan write was rejected".into()));
                    }
                    if kind != "noop" {
                        writer.execute("INSERT INTO t VALUES (1)", []).unwrap();
                    }
                    Ok(())
                };
                let result = if kind == "quiet" {
                    tracker.record_own_write_quiet(write, || data_version(&writer))
                } else {
                    tracker.record_own_write_if(
                        write,
                        |_| kind == "ordinary",
                        || data_version(&writer),
                    )
                };
                if kind == "failed" {
                    assert!(result.is_err());
                    if holding {
                        assert_eq!(
                            tracker.poll().unwrap().revision,
                            start,
                            "rejected write keeps the hold"
                        );
                        assert!(tracker.end_hold(true).unwrap().revision > start);
                    } else {
                        assert!(tracker.poll().unwrap().revision > start);
                    }
                } else {
                    let outcome = result.unwrap();
                    assert!(outcome.revision_before > start, "{holding}/{kind}");
                    assert_eq!(
                        outcome.revision - outcome.revision_before,
                        u64::from(kind == "ordinary")
                    );
                    if holding {
                        assert!(tracker.end_hold(true).unwrap().revision > outcome.revision);
                    } else {
                        assert_eq!(tracker.poll().unwrap().revision, outcome.revision);
                    }
                }
            }
        }
    }

    #[test]
    fn uncertain_own_commits_survive_an_unchanged_scan_hold_end() {
        use std::cell::Cell;
        for quiet in [false, true] {
            for failure in ["operation", "observer", "sidecar"] {
                let dir = tempfile::tempdir().unwrap();
                let (path, _setup) = database(dir.path());
                let mut tracker = RevisionTracker::open(&path).unwrap();
                let writer = wal_writer(&path);
                let start = tracker.poll().unwrap().revision;
                let sidecar = tracker.sidecar.clone();
                if failure == "sidecar" {
                    // An owned directory cannot be replaced by the state file. Restore
                    // the real target before finishing the hold so recovery can persist.
                    tracker.sidecar = dir.path().join("blocked-sidecar");
                    fs::create_dir(&tracker.sidecar).unwrap();
                }
                tracker.begin_hold().unwrap();
                let observations = Cell::new(0);
                let write = || {
                    writer.execute("INSERT INTO t VALUES (1)", []).unwrap();
                    if failure == "operation" {
                        Err(Error::Database("failure after commit".into()))
                    } else {
                        Ok(())
                    }
                };
                let observe = || {
                    let count = observations.get();
                    observations.set(count + 1);
                    if failure == "observer" && count == 1 {
                        Err(Error::Database("final observation failed".into()))
                    } else {
                        data_version(&writer)
                    }
                };
                // A quiet write normally leaves its baseline to end_hold. For the
                // sidecar case, a foreign commit forces immediate state persistence.
                let external = wal_writer(&path);
                let operation = || {
                    if quiet && failure == "sidecar" {
                        external.execute("INSERT INTO t VALUES (2)", []).unwrap();
                    }
                    write()
                };
                let result = if quiet {
                    tracker.record_own_write_quiet(operation, observe)
                } else {
                    tracker.record_own_write(operation, observe)
                };
                assert!(result.is_err(), "{quiet}/{failure}");
                assert_eq!(
                    tracker.poll().unwrap().revision,
                    start,
                    "failure must retain the active hold"
                );
                tracker.sidecar = sidecar;
                let finished = tracker.end_hold(false).unwrap();
                assert!(
                    finished.revision > start,
                    "{quiet}/{failure} must not absorb a possible business commit"
                );
                assert_eq!(tracker.poll().unwrap(), finished);
                drop(tracker);
                assert_eq!(
                    RevisionTracker::open(&path).unwrap().poll().unwrap(),
                    finished
                );
            }
        }
    }

    #[test]
    fn successful_or_unattempted_writes_do_not_dirty_an_unchanged_hold() {
        for kind in ["ordinary", "quiet", "noop", "initial_observer_error"] {
            let dir = tempfile::tempdir().unwrap();
            let (path, _setup) = database(dir.path());
            let tracker = RevisionTracker::open(&path).unwrap();
            let writer = wal_writer(&path);
            let start = tracker.poll().unwrap().revision;
            tracker.begin_hold().unwrap();
            let result = if kind == "quiet" {
                tracker.record_own_write_quiet(
                    || {
                        writer
                            .execute("INSERT INTO t VALUES (1)", [])
                            .map_err(db_error)
                    },
                    || data_version(&writer),
                )
            } else {
                tracker.record_own_write_if(
                    || {
                        assert_ne!(
                            kind, "initial_observer_error",
                            "must fail before the operation"
                        );
                        if kind == "ordinary" {
                            writer
                                .execute("INSERT INTO t VALUES (1)", [])
                                .map_err(db_error)
                        } else {
                            Ok(0)
                        }
                    },
                    |_| kind == "ordinary",
                    || {
                        if kind == "initial_observer_error" {
                            Err(Error::Database("initial observation failed".into()))
                        } else {
                            data_version(&writer)
                        }
                    },
                )
            };
            if kind == "initial_observer_error" {
                assert!(result.is_err());
            } else {
                result.unwrap();
            }
            let expected = start + u64::from(kind == "ordinary");
            assert_eq!(
                tracker.end_hold(false).unwrap().revision,
                expected,
                "{kind}"
            );
            assert_eq!(tracker.poll().unwrap().revision, expected);
        }
    }

    #[test]
    fn corrupt_sidecar_is_reported_instead_of_regenerated() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _writer) = database(dir.path());
        fs::write(sidecar_path(&path), b"{not json").unwrap();
        assert!(matches!(
            RevisionTracker::open(&path),
            Err(Error::Validation(_))
        ));
    }
}
