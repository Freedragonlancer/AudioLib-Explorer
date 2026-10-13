// crates/audiolib-remote/src/sync.rs
//! Revision-checked snapshot cache for server mode.
//!
//! Layout under `<data dir>/remote/`:
//! * `active.json` - the library_id last synced successfully (offline startup).
//! * `<library_id>/cache.json` - [`CacheState`]: revision, snapshot file, roots.
//! * `<library_id>/library-r<revision>-<millis>.db` - validated read-only snapshot.
//!   A new file is used per download because Windows cannot replace an open SQLite
//!   file. [`sync_library`] never deletes anything: the desktop calls
//!   [`RemoteCache::cleanup_after_swap`] once its provider has switched to the new file.
//! * `<library_id>/audio-cache/` - the disk audition cache of earlier versions (audition
//!   now streams into memory); [`RemoteCache::remove_audition_caches`] deletes it once.
use crate::client::{RemoteClient, RemoteLibrary, RemoteRoot, SnapshotMeta};
use crate::error::{RemoteError, Result};
use crate::util::{fnv64, unix_now, write_atomic};
#[cfg(test)]
use audiolib_core::DataProvider;
use audiolib_core::server_url::ServerAddress;
use audiolib_db::{
    MAX_SNAPSHOT_SCHEMA, MIN_SNAPSHOT_SCHEMA, SqliteProvider, snapshot_schema_supported,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const STATE_FILE: &str = "cache.json";
const ACTIVE_FILE: &str = "active.json";
/// How long a snapshot download keeps retrying `409 BUSY` / `503 BUSY`, measured from the
/// first request. A 900k-row library needs ~125 s for its first snapshot and the server
/// answers the first request with `503 BUSY` (+ `Retry-After`) after ~90 s, so the window
/// must clearly exceed 180 s (the retry waits come from `Retry-After`, capped at 10 s).
const BUSY_WINDOW: Duration = Duration::from_secs(240);
/// Retries of a snapshot download that timed out or lost its connection.
const TRANSIENT_RETRIES: u32 = 3;
/// A snapshot staging file (`.download`, `.download.part-<pid>`, legacy `.db.part-<pid>`)
/// untouched this long belongs to a sync that crashed or was killed.
const STALE_STAGING_AGE: Duration = Duration::from_secs(60 * 60);

/// The same snapshot compatibility gate for sync, polling and connection diagnostics.
pub(crate) fn snapshot_schema_error(version: i64, chinese: bool) -> Option<String> {
    if snapshot_schema_supported(version) {
        return None;
    }
    Some(if version > MAX_SNAPSHOT_SCHEMA {
        if chinese {
            format!(
                "服务器素材库 schema {version} 高于本客户端支持的版本（{MAX_SNAPSHOT_SCHEMA}），请更新 AudioLib Explorer"
            )
        } else {
            format!(
                "server library schema {version} is newer than this client supports ({MAX_SNAPSHOT_SCHEMA}); update AudioLib Explorer"
            )
        }
    } else if chinese {
        format!(
            "服务器素材库 schema {version} 不受本客户端支持（支持 {MIN_SNAPSHOT_SCHEMA}..={MAX_SNAPSHOT_SCHEMA}），请升级服务器素材库"
        )
    } else {
        format!(
            "server library schema {version} is not supported by this client (supported {MIN_SNAPSHOT_SCHEMA}..={MAX_SNAPSHOT_SCHEMA}); upgrade the server library"
        )
    })
}

/// Wait before BUSY retry `attempt` (1-based): 0.5, 1, 2, 4 s, as documented in §11.
fn busy_backoff(attempt: u32) -> Duration {
    Duration::from_millis(500u64 << attempt.saturating_sub(1).min(5))
}

/// Longest wait for one BUSY retry, whatever `Retry-After` asks for.
const MAX_BUSY_WAIT: Duration = Duration::from_secs(10);

/// The BUSY wait: the server's `Retry-After` when sent, else [`busy_backoff`].
fn busy_wait(attempt: u32, retry_after: Option<u64>) -> Duration {
    crate::error::retry_wait(busy_backoff(attempt), retry_after, MAX_BUSY_WAIT)
}

/// Wait before retry `attempt` (1-based) of a timed-out or interrupted snapshot
/// download: 2, 5, 10 s.
fn transient_backoff(attempt: u32) -> Duration {
    if cfg!(test) {
        return Duration::from_millis(10);
    }
    Duration::from_secs([2, 5, 10][(attempt.saturating_sub(1) as usize).min(2)])
}

/// Sleep `wait` in small slices, returning early with `Cancelled` when `cancel` is set.
fn wait_unless_cancelled(wait: Duration, cancel: &AtomicBool) -> Result<()> {
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        if cancel.load(Ordering::Acquire) {
            return Err(RemoteError::Cancelled);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Longest continuous run of Busy/Locked backup steps tolerated while preparing a cache.
const BACKUP_BUSY_RETRY_LIMIT: Duration = Duration::from_secs(5);

/// Classified outcome of one `sqlite3_backup_step`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BackupStep {
    Done,
    More,
    Busy,
    Unknown(String),
}

impl From<rusqlite::backup::StepResult> for BackupStep {
    fn from(step: rusqlite::backup::StepResult) -> Self {
        use rusqlite::backup::StepResult;
        match step {
            StepResult::Done => Self::Done,
            StepResult::More => Self::More,
            StepResult::Busy | StepResult::Locked => Self::Busy,
            #[allow(unreachable_patterns)]
            other => Self::Unknown(format!("{other:?}")),
        }
    }
}

/// Run backup steps until done, checking `cancel` before and after every step.
/// Busy/Locked steps are retried for at most `busy_limit` in a row; an unknown step
/// result is an error instead of an unbounded retry.
fn drive_backup_steps(
    cancel: &AtomicBool,
    busy_limit: Duration,
    step: &mut dyn FnMut() -> Result<BackupStep>,
) -> Result<()> {
    let mut busy_since: Option<Instant> = None;
    loop {
        check_cancelled(cancel)?;
        let result = step()?;
        check_cancelled(cancel)?;
        match result {
            BackupStep::Done => return Ok(()),
            BackupStep::More => busy_since = None,
            BackupStep::Busy => {
                let since = *busy_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= busy_limit {
                    return Err(RemoteError::Snapshot(format!(
                        "cache snapshot copy stayed busy or locked for {} ms",
                        busy_limit.as_millis()
                    )));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            BackupStep::Unknown(result) => {
                return Err(RemoteError::Snapshot(format!(
                    "unexpected SQLite backup step result: {result}"
                )));
            }
        }
    }
}

fn check_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(RemoteError::Cancelled)
    } else {
        Ok(())
    }
}

/// Fetch `GET /roots`, returning them even if the current library is not confirmed.
/// Persist roots only when `confirmed_library` came from this client's successful
/// `GET /library` and the active cache belongs to that library and API endpoint.
pub fn refresh_cached_roots(
    client: &RemoteClient,
    cache: &RemoteCache,
    confirmed_library: Option<&RemoteLibrary>,
) -> Option<Vec<RemoteRoot>> {
    let roots = client.roots().ok()?;
    if let Some(library) = confirmed_library
        && snapshot_schema_supported(library.schema_version)
        && let Some(mut state) = cache.active()
        && state.library_id == library.library_id
        && same_api_endpoint(&state.server_url, client.api_url())
    {
        state.roots = roots.clone();
        if let Err(error) = cache.save_state(&state) {
            tracing::warn!(%error, "Could not store the refreshed server roots");
        }
    }
    Some(roots)
}

fn same_api_endpoint(cached: &str, current: &str) -> bool {
    match (ServerAddress::parse(cached), ServerAddress::parse(current)) {
        (Ok(cached), Ok(current)) => {
            cached.scheme == current.scheme
                && cached.host.eq_ignore_ascii_case(&current.host)
                && cached.effective_port() == current.effective_port()
                && cached.prefix == current.prefix
        }
        _ => false,
    }
}

/// Persisted description of one cached server library.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CacheState {
    pub library_id: String,
    pub revision: u64,
    pub schema_version: i64,
    /// Snapshot file name inside the library cache directory.
    pub database_file: String,
    pub server_url: String,
    #[serde(default)]
    pub server_version: String,
    /// Unix seconds when the current snapshot was downloaded.
    #[serde(default)]
    pub downloaded_at: u64,
    /// Unix seconds of the last successful revision check.
    #[serde(default)]
    pub checked_at: u64,
    #[serde(default)]
    pub snapshot_bytes: u64,
    #[serde(default)]
    pub audio_files: u64,
    /// Last known server scan roots, so WebDAV mapping works while the server is down.
    #[serde(default)]
    pub roots: Vec<RemoteRoot>,
}

#[derive(Serialize, Deserialize)]
struct ActiveLibrary {
    library_id: String,
}

/// Progress callbacks emitted by [`sync_library`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncProgress {
    Checking,
    Downloading { received: u64, total: Option<u64> },
    Validating,
    Preparing(CachePreparation),
    Saving,
}

/// Offline cache preparation stages. Copy counts are SQLite pages, not total startup progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CachePreparation {
    Checking,
    Copying { completed: u64, total: u64 },
    Validating,
    Upgrading,
    Saving,
}

/// Result of one sync.
#[derive(Clone, Debug)]
pub struct SyncOutcome {
    pub state: CacheState,
    /// A new snapshot was downloaded and validated.
    pub downloaded: bool,
    /// The previously active cached library had a different library_id.
    pub library_changed: bool,
    pub previous: Option<CacheState>,
    pub elapsed_ms: u64,
}

/// Owner of `<data dir>/remote`.
#[derive(Clone, Debug)]
pub struct RemoteCache {
    root: PathBuf,
}

/// File-system-safe directory name for a server library identity.
pub(crate) fn directory_name(library_id: &str) -> String {
    let mut name: String = library_id
        .chars()
        .take(80)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name != library_id || name.is_empty() {
        name.push_str(&format!("-{:016x}", fnv64(library_id)));
    }
    name
}

/// Smallest id of a record that is not marked missing; read-only, never creates the file.
fn first_present_audio_id(database: &Path) -> Option<i64> {
    let connection = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    connection
        .query_row(
            "SELECT id FROM audio_files WHERE missing=0 ORDER BY id LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok()
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

impl RemoteCache {
    /// Cache rooted at `<data_root>/remote`.
    pub fn new(data_root: &Path) -> Self {
        Self {
            root: data_root.join("remote"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory for one library's snapshot, state and audio cache.
    pub fn library_dir(&self, library_id: &str) -> PathBuf {
        self.root.join(directory_name(library_id))
    }

    /// Former disk audition cache directory of one library (see
    /// [`Self::remove_audition_caches`]).
    pub fn audio_cache_dir(&self, library_id: &str) -> PathBuf {
        self.library_dir(library_id).join("audio-cache")
    }

    /// Delete the disk audition caches (`<library>/audio-cache/`) of earlier versions in
    /// every library folder; audition streams into memory now. Drag-out downloads live
    /// elsewhere (`<data dir>/cache/`) and are not touched. Returns the folders removed.
    pub fn remove_audition_caches(&self) -> usize {
        let Ok(libraries) = std::fs::read_dir(&self.root) else {
            return 0;
        };
        let mut removed = 0;
        for library in libraries.flatten() {
            let directory = library.path().join("audio-cache");
            let is_dir =
                std::fs::symlink_metadata(&directory).is_ok_and(|metadata| metadata.is_dir());
            if is_dir && std::fs::remove_dir_all(&directory).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// Absolute path of a state's snapshot database.
    pub fn database_path(&self, state: &CacheState) -> PathBuf {
        self.library_dir(&state.library_id)
            .join(&state.database_file)
    }

    /// Cached state for `library_id` whose snapshot file still exists.
    pub fn state(&self, library_id: &str) -> Option<CacheState> {
        let state: CacheState = read_json(&self.library_dir(library_id).join(STATE_FILE))?;
        (state.library_id == library_id
            && !state.database_file.contains(['/', '\\'])
            && self.database_path(&state).is_file())
        .then_some(state)
    }

    /// Id of one present (not missing) record in the synced snapshot of `library_id`,
    /// used by the connection test to check `/audio/{id}/stream`.
    pub fn sample_audio_id(&self, library_id: &str) -> Option<i64> {
        let state = self.state(library_id)?;
        first_present_audio_id(&self.database_path(&state))
    }

    /// The last successfully synced library, used for offline startup.
    pub fn active(&self) -> Option<CacheState> {
        self.state(&self.active_library_id()?)
    }

    /// The library id `active.json` names, even when its snapshot file is gone.
    pub fn active_library_id(&self) -> Option<String> {
        read_json::<ActiveLibrary>(&self.root.join(ACTIVE_FILE)).map(|active| active.library_id)
    }

    /// Upgrade an older cached search index in an owned copy, keeping the original
    /// snapshot available to any already-open read-only provider. Works offline and
    /// never opens or modifies the user's local working library.
    pub fn prepare_cached_state(&self, state: &CacheState) -> Result<CacheState> {
        self.prepare_cached_state_with_progress(state, &mut |_| {})
    }

    /// Prepare an offline cache with synchronous, lightweight worker-thread callbacks.
    /// Unknown-duration validation/index stages intentionally have no percentage.
    pub fn prepare_cached_state_with_progress(
        &self,
        state: &CacheState,
        progress: &mut dyn FnMut(CachePreparation),
    ) -> Result<CacheState> {
        self.prepare_cached_state_with_progress_cancellable(
            state,
            progress,
            &AtomicBool::new(false),
        )
    }

    /// Prepare a cache in an owned copy, cancelling copy, validation and index SQL.
    /// Cancellation removes only this preparation's staging file and preserves the
    /// active cache and any read-only provider which already owns its snapshot.
    pub fn prepare_cached_state_with_progress_cancellable(
        &self,
        state: &CacheState,
        progress: &mut dyn FnMut(CachePreparation),
        cancel: &AtomicBool,
    ) -> Result<CacheState> {
        check_cancelled(cancel)?;
        progress(CachePreparation::Checking);
        check_cancelled(cancel)?;
        let source = self.database_path(state);
        if SqliteProvider::has_current_search_state(&source)? {
            check_cancelled(cancel)?;
            return Ok(state.clone());
        }
        let directory = self.library_dir(&state.library_id);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let name = format!("library-r{}-{nonce}.db", state.revision);
        let staging = directory.join(format!("{name}.download"));
        // Reserve our unique staging file before SQLite opens it. Error cleanup may
        // remove only this owned file and its SQLite sidecars.
        let reserved = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        drop(reserved);
        let result = (|| -> Result<audiolib_core::LibraryInfo> {
            let source = rusqlite::Connection::open_with_flags(
                &source,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|error| RemoteError::Snapshot(error.to_string()))?;
            let mut target = rusqlite::Connection::open_with_flags(
                &staging,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            )
            .map_err(|error| RemoteError::Snapshot(error.to_string()))?;
            // The staging file is owned by this preparation: never wait inside SQLite
            // for its lock, so Busy/Locked is reported per step and bounded below.
            target
                .busy_timeout(Duration::ZERO)
                .map_err(|error| RemoteError::Snapshot(error.to_string()))?;
            // Pin one consistent read snapshot, including committed WAL contents.
            // This owned startup copy needs no per-step throttle: at 4 KiB pages,
            // sleeping 5 ms per 256 pages adds about 50 seconds to a 10 GiB cache.
            source
                .execute_batch("BEGIN; SELECT COUNT(*) FROM sqlite_schema;")
                .map_err(|error| RemoteError::Snapshot(error.to_string()))?;
            progress(CachePreparation::Copying {
                completed: 0,
                total: 0,
            });
            let backup = rusqlite::backup::Backup::new(&source, &mut target)
                .map_err(|error| RemoteError::Snapshot(error.to_string()))?;
            drive_backup_steps(cancel, BACKUP_BUSY_RETRY_LIMIT, &mut || {
                let step = backup
                    .step(4096)
                    .map_err(|error| RemoteError::Snapshot(error.to_string()))?;
                let copied = backup.progress();
                progress(CachePreparation::Copying {
                    completed: (copied.pagecount - copied.remaining).max(0) as u64,
                    total: copied.pagecount.max(0) as u64,
                });
                Ok(BackupStep::from(step))
            })?;
            drop(backup);
            drop(target);
            drop(source);
            validate_snapshot_with_progress_cancellable(&staging, progress, cancel)
        })();
        let info = match result {
            Ok(info) => info,
            Err(error) => {
                remove_owned_staging(&staging);
                return Err(error);
            }
        };
        progress(CachePreparation::Saving);
        if let Err(error) = check_cancelled(cancel) {
            remove_owned_staging(&staging);
            return Err(error);
        }
        let target = directory.join(&name);
        if let Err(error) = std::fs::rename(&staging, &target) {
            remove_owned_staging(&staging);
            return Err(error.into());
        }
        let mut prepared = state.clone();
        prepared.database_file = name;
        prepared.schema_version = info.schema_version;
        prepared.audio_files = info.stats.audio_files;
        prepared.snapshot_bytes = std::fs::metadata(&target)?.len();
        if let Err(error) = check_cancelled(cancel) {
            remove_owned_staging(&target);
            return Err(error);
        }
        self.save_state(&prepared)?;
        Ok(prepared)
    }

    /// Persist state and mark it active.
    pub fn save_state(&self, state: &CacheState) -> Result<()> {
        let directory = self.library_dir(&state.library_id);
        let json = serde_json::to_vec_pretty(state)
            .map_err(|error| RemoteError::Protocol(error.to_string()))?;
        write_atomic(&directory.join(STATE_FILE), &json)?;
        let active = serde_json::to_vec_pretty(&ActiveLibrary {
            library_id: state.library_id.clone(),
        })
        .map_err(|error| RemoteError::Protocol(error.to_string()))?;
        write_atomic(&self.root.join(ACTIVE_FILE), &active)?;
        Ok(())
    }

    /// An empty, schema-current, read-only-openable library shown in server mode before
    /// the first successful sync. Never the user's local library.
    pub fn placeholder_database(&self) -> Result<PathBuf> {
        let path = self.root.join("placeholder").join("library.db");
        if !path.is_file() {
            drop(SqliteProvider::open(&path)?);
            let conn = rusqlite::Connection::open(&path)
                .map_err(|error| RemoteError::Snapshot(error.to_string()))?;
            conn.query_row("PRAGMA journal_mode=DELETE", [], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| RemoteError::Snapshot(error.to_string()))?;
        }
        Ok(path)
    }

    /// Delete snapshot files of this library other than `keep` (best effort: files still
    /// open by a reader cannot be deleted on Windows and are retried next time). Staging
    /// files of a download are removed only once stale ([`STALE_STAGING_AGE`]), so a sync
    /// running in another process is never disturbed.
    pub fn prune(&self, library_id: &str, keep: &[&str]) {
        self.prune_files(library_id, keep, true);
    }

    /// Remove only this library's stale staging files (older than [`STALE_STAGING_AGE`]),
    /// left behind by a preparation or download that was abandoned, for example by a
    /// forced exit. Snapshots are never touched, so this is safe before any swap.
    pub fn prune_stale_staging(&self, library_id: &str) {
        self.prune_files(library_id, &[], false);
    }

    fn prune_files(&self, library_id: &str, keep: &[&str], snapshots: bool) {
        let Ok(entries) = std::fs::read_dir(self.library_dir(library_id)) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let base = name
                .strip_suffix("-journal")
                .or_else(|| name.strip_suffix("-wal"))
                .or_else(|| name.strip_suffix("-shm"))
                .unwrap_or(&name);
            if !base.starts_with("library-r") || keep.contains(&base) {
                continue;
            }
            let staging = base.contains(".download") || base.contains(".db.part-");
            if staging {
                let stale = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > STALE_STAGING_AGE);
                if stale {
                    let _ = std::fs::remove_file(entry.path());
                }
            } else if snapshots && base.ends_with(".db") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// Clean up after the desktop's provider switched to `opened`: delete this library's
    /// other snapshots and the libraries this server replaced. Call it only after a
    /// successful swap. Nothing is deleted while `active.json` names another library (a
    /// newer sync waits for its swap), and both the opened snapshot and the one
    /// `cache.json` records are always kept.
    pub fn cleanup_after_swap(&self, opened: &CacheState) {
        if self.active_library_id().as_deref() != Some(opened.library_id.as_str()) {
            return;
        }
        let recorded = self
            .state(&opened.library_id)
            .map(|state| state.database_file);
        let mut keep = vec![opened.database_file.as_str()];
        if let Some(recorded) = recorded.as_deref() {
            keep.push(recorded);
        }
        self.prune(&opened.library_id, &keep);
        self.discard_replaced(opened);
    }
}

/// Delete every file under `directory` except `keep_name` at its top level, then empty
/// directories; best effort (open files stay). `true` when only `keep_name` is left.
fn remove_contents(directory: &Path, keep_name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    let mut clean = true;
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name() == keep_name {
            continue;
        }
        let removed = match entry.file_type() {
            Ok(kind) if kind.is_dir() => {
                remove_contents(&path, "") && std::fs::remove_dir(&path).is_ok()
            }
            Ok(_) => std::fs::remove_file(&path).is_ok(),
            Err(_) => false,
        };
        clean &= removed;
    }
    clean
}

impl RemoteCache {
    /// Remove the cached data of libraries this server no longer serves: every library
    /// directory whose `cache.json` names the same `server_url` as `active` but another
    /// `library_id` (the server's database was replaced). Its snapshots, state and
    /// playback audio cache are deleted; libraries of other servers are kept (switching
    /// servers back and forth needs no new download), and the drag-out cache
    /// (`<data dir>/cache`) is left to its own LRU eviction. Files still open (the old
    /// snapshot right before a swap, on Windows) stay with their `cache.json`, so a later
    /// call finishes the job. Returns the library ids removed completely.
    pub fn discard_replaced(&self, active: &CacheState) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        // Never the library `active.json` names (a sync may have just installed it),
        // whatever `active` the caller passes.
        let recorded = self
            .active_library_id()
            .map(|library_id| self.library_dir(&library_id));
        let mut removed = Vec::new();
        for entry in entries.flatten() {
            let directory = entry.path();
            if !directory.is_dir()
                || directory == self.library_dir(&active.library_id)
                || recorded.as_ref() == Some(&directory)
            {
                continue;
            }
            let Some(state) = read_json::<CacheState>(&directory.join(STATE_FILE)) else {
                continue;
            };
            if state.server_url != active.server_url
                || state.library_id == active.library_id
                || directory != self.library_dir(&state.library_id)
            {
                continue;
            }
            if remove_contents(&directory, STATE_FILE)
                && std::fs::remove_file(directory.join(STATE_FILE)).is_ok()
                && std::fs::remove_dir(&directory).is_ok()
            {
                tracing::info!(library_id = %state.library_id, "Removed replaced server library cache");
                removed.push(state.library_id);
            } else {
                tracing::info!(library_id = %state.library_id, "Replaced server library cache still in use; retrying later");
            }
        }
        removed
    }
}

/// Validate a downloaded snapshot without trusting it: real SQLite, AudioLib identity,
/// supported schema, quick_check, foreign keys and FTS5 consistency. Converts the file
/// to rollback-journal mode so it opens read-only without WAL sidecars.
pub fn validate_snapshot(path: &Path) -> Result<audiolib_core::LibraryInfo> {
    validate_snapshot_with_progress(path, &mut |_| {})
}

fn validate_snapshot_with_progress(
    path: &Path,
    progress: &mut dyn FnMut(CachePreparation),
) -> Result<audiolib_core::LibraryInfo> {
    validate_snapshot_with_progress_cancellable(path, progress, &AtomicBool::new(false))
}

fn validate_snapshot_with_progress_cancellable(
    path: &Path,
    progress: &mut dyn FnMut(CachePreparation),
    cancel: &AtomicBool,
) -> Result<audiolib_core::LibraryInfo> {
    check_cancelled(cancel)?;
    progress(CachePreparation::Validating);
    check_cancelled(cancel)?;
    {
        let conn = rusqlite::Connection::open(path)
            .map_err(|error| RemoteError::Snapshot(error.to_string()))?;
        conn.query_row("PRAGMA journal_mode=DELETE", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|error| RemoteError::Snapshot(format!("not a SQLite database: {error}")))?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| RemoteError::Snapshot(error.to_string()))?;
        if !snapshot_schema_supported(version) {
            return Err(RemoteError::Snapshot(format!(
                "schema {version} is not supported by this client (expected {MIN_SNAPSHOT_SCHEMA}..={MAX_SNAPSHOT_SCHEMA})"
            )));
        }
    }
    // The staging file belongs to this sync alone: check it in place (one quick_check, no
    // temporary copy of a possibly several-hundred-MB library).
    let report = SqliteProvider::check_owned_file_cancellable(path, cancel)?;
    if !report.is_ok() {
        return Err(RemoteError::Snapshot(format!(
            "integrity check failed: {}",
            serde_json::to_string(&report).unwrap_or_default()
        )));
    }
    // Validate the downloaded bytes before repairing derived state, so an unexpected
    // or missing FTS trigger cannot be hidden by a successful rebuild. This file is
    // owned staging, not a live/user library; upgrade without creating WAL sidecars.
    progress(CachePreparation::Upgrading);
    check_cancelled(cancel)?;
    if SqliteProvider::upgrade_owned_snapshot_cancellable(path, cancel)? {
        progress(CachePreparation::Validating);
        check_cancelled(cancel)?;
        let upgraded = SqliteProvider::check_owned_file_cancellable(path, cancel)?;
        if !upgraded.is_ok() {
            return Err(RemoteError::Snapshot(format!(
                "upgraded snapshot integrity check failed: {}",
                serde_json::to_string(&upgraded).unwrap_or_default()
            )));
        }
    }
    check_cancelled(cancel)?;
    SqliteProvider::owned_snapshot_info_cancellable(path, cancel).map_err(Into::into)
}

fn remove_owned_staging(staging: &Path) {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut path = staging.as_os_str().to_os_string();
        path.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(path));
    }
}

/// Bring the local cache up to the server's revision.
///
/// `GET /library`; when no cache exists for its `library_id`, or the cached revision
/// differs, or `force` is set, download `GET /library/snapshot`, validate it and record
/// the new state. Roots are refreshed on every successful check. On any failure the
/// existing cache and active library remain unchanged. Nothing is deleted here: the
/// previous snapshot (possibly still open) stays until
/// [`RemoteCache::cleanup_after_swap`] runs after the provider swap.
pub fn sync_library(
    client: &RemoteClient,
    cache: &RemoteCache,
    progress: &mut dyn FnMut(SyncProgress),
    cancel: &AtomicBool,
    force: bool,
) -> Result<SyncOutcome> {
    let started = Instant::now();
    progress(SyncProgress::Checking);
    let server_version = match client.health_cancellable(cancel) {
        Ok(health) => health.version,
        Err(RemoteError::Cancelled) => return Err(RemoteError::Cancelled),
        Err(_) => String::new(),
    };
    let library: RemoteLibrary = match client.library_cancellable(cancel) {
        Ok(library) => library,
        Err(error) => {
            // `/roots` may still answer when `/library` is slow, but this response
            // cannot update a cache without confirming its library identity.
            if error.is_slow() {
                // No identity was confirmed, so these roots cannot be persisted.
                if matches!(
                    client.roots_cancellable(cancel),
                    Err(RemoteError::Cancelled)
                ) {
                    return Err(RemoteError::Cancelled);
                }
            }
            return Err(error);
        }
    };
    if let Some(message) = snapshot_schema_error(library.schema_version, false) {
        return Err(RemoteError::Snapshot(message));
    }
    let roots = match client.roots_cancellable(cancel) {
        Ok(roots) => Some(roots),
        Err(error) if error.is_unauthorized() || matches!(error, RemoteError::Cancelled) => {
            return Err(error);
        }
        Err(error) => {
            tracing::warn!(%error, "Server roots unavailable; keeping cached roots");
            None
        }
    };
    let previous = cache.active();
    let library_changed = previous
        .as_ref()
        .is_some_and(|state| state.library_id != library.library_id);
    let existing = cache.state(&library.library_id);
    let now = unix_now();
    if !force
        && let Some(mut state) = existing
            .clone()
            .filter(|state| state.revision == library.revision)
    {
        state = cache.prepare_cached_state_with_progress_cancellable(
            &state,
            &mut |stage| progress(SyncProgress::Preparing(stage)),
            cancel,
        )?;
        if let Some(roots) = roots {
            state.roots = roots;
        }
        state.server_url = client.api_url().to_owned();
        state.server_version = server_version;
        state.checked_at = now;
        if cancel.load(Ordering::Acquire) {
            return Err(RemoteError::Cancelled);
        }
        progress(SyncProgress::Saving);
        if cancel.load(Ordering::Acquire) {
            return Err(RemoteError::Cancelled);
        }
        cache.save_state(&state)?;
        return Ok(SyncOutcome {
            state,
            downloaded: false,
            library_changed,
            previous,
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
    }
    let directory = cache.library_dir(&library.library_id);
    std::fs::create_dir_all(&directory)?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let file_name = format!("library-r{}-{millis}.db", library.revision);
    let staging = directory.join(format!("{file_name}.download"));
    let result = (|| -> Result<(SnapshotMeta, audiolib_core::LibraryInfo)> {
        progress(SyncProgress::Downloading {
            received: 0,
            total: None,
        });
        // The server prepares at most two snapshots at once and answers 409 BUSY beyond
        // that, and 503 BUSY (with Retry-After) when a large first snapshot is still being
        // generated; retry on the server's Retry-After (else a short exponential backoff)
        // until BUSY_WINDOW has passed.
        let busy_started = Instant::now();
        let mut attempt = 0;
        let mut transient = 0;
        let meta = loop {
            match client.download_snapshot(
                &staging,
                &mut |received, total| progress(SyncProgress::Downloading { received, total }),
                cancel,
            ) {
                Err(RemoteError::Busy {
                    message,
                    retry_after,
                }) if busy_started.elapsed() < BUSY_WINDOW => {
                    attempt += 1;
                    tracing::info!(attempt, %message, ?retry_after, "Snapshot slots busy; retrying");
                    wait_unless_cancelled(busy_wait(attempt, retry_after), cancel)?;
                }
                // A timeout or a lost connection is retried a few times (the server may be
                // busy preparing the snapshot of a large library); a cancel or a rejected
                // snapshot is not.
                Err(error)
                    if (error.is_slow() || error.is_unreachable())
                        && transient < TRANSIENT_RETRIES =>
                {
                    transient += 1;
                    tracing::info!(transient, %error, "Snapshot download interrupted; retrying");
                    wait_unless_cancelled(transient_backoff(transient), cancel)?;
                }
                other => break other?,
            }
        };
        if meta
            .library_id
            .as_deref()
            .is_some_and(|id| id != library.library_id)
        {
            return Err(RemoteError::Snapshot(
                "the server library changed during download; sync again".into(),
            ));
        }
        if cancel.load(Ordering::Acquire) {
            return Err(RemoteError::Cancelled);
        }
        progress(SyncProgress::Validating);
        let info = validate_snapshot_with_progress_cancellable(
            &staging,
            &mut |stage| progress(SyncProgress::Preparing(stage)),
            cancel,
        )?;
        Ok((meta, info))
    })();
    let (meta, info) = match result {
        Ok(value) => value,
        Err(error) => {
            remove_owned_staging(&staging);
            return Err(error);
        }
    };
    if cancel.load(Ordering::Acquire) {
        remove_owned_staging(&staging);
        return Err(RemoteError::Cancelled);
    }
    let target = directory.join(&file_name);
    std::fs::rename(&staging, &target)?;
    let state = CacheState {
        library_id: library.library_id.clone(),
        // The snapshot header names the exact revision it was taken at (possibly newer
        // than the earlier /library reply); fall back to the /library revision.
        revision: meta.revision.unwrap_or(library.revision),
        schema_version: info.schema_version,
        database_file: file_name.clone(),
        server_url: client.api_url().to_owned(),
        server_version,
        downloaded_at: now,
        checked_at: now,
        // A local derived-index upgrade may change the SQLite file size after download.
        snapshot_bytes: std::fs::metadata(&target)?.len(),
        audio_files: info.stats.audio_files,
        roots: roots
            .or_else(|| existing.as_ref().map(|state| state.roots.clone()))
            .unwrap_or_default(),
    };
    progress(SyncProgress::Saving);
    if cancel.load(Ordering::Acquire) {
        remove_owned_staging(&target);
        return Err(RemoteError::Cancelled);
    }
    cache.save_state(&state)?;
    Ok(SyncOutcome {
        state,
        downloaded: true,
        library_changed,
        previous,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn backup_busy_beyond_limit_is_an_error() {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut steps = 0;
        let started = std::time::Instant::now();
        let error =
            super::drive_backup_steps(&cancel, std::time::Duration::from_millis(60), &mut || {
                steps += 1;
                Ok(super::BackupStep::Busy)
            })
            .unwrap_err();
        assert!(
            matches!(error, super::RemoteError::Snapshot(ref m) if m.contains("busy")),
            "{error:?}"
        );
        assert!(steps > 1, "busy steps are retried");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        // Progress between busy steps restarts the window; Done finishes.
        let mut script = vec![
            super::BackupStep::Done,
            super::BackupStep::More,
            super::BackupStep::Busy,
        ];
        super::drive_backup_steps(&cancel, std::time::Duration::from_secs(5), &mut || {
            Ok(script.pop().unwrap())
        })
        .unwrap();
    }

    #[test]
    fn unknown_backup_step_is_an_error_not_a_retry() {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut steps = 0;
        let error =
            super::drive_backup_steps(&cancel, std::time::Duration::from_secs(5), &mut || {
                steps += 1;
                Ok(super::BackupStep::Unknown("Future".into()))
            })
            .unwrap_err();
        assert_eq!(steps, 1);
        assert!(
            matches!(error, super::RemoteError::Snapshot(ref m) if m.contains("Future")),
            "{error:?}"
        );
    }

    #[test]
    fn backup_steps_observe_cancellation_after_each_step() {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut steps = 0;
        let error =
            super::drive_backup_steps(&cancel, std::time::Duration::from_secs(5), &mut || {
                steps += 1;
                cancel.store(true, std::sync::atomic::Ordering::Release);
                Ok(super::BackupStep::More)
            })
            .unwrap_err();
        assert!(matches!(error, super::RemoteError::Cancelled));
        assert_eq!(steps, 1);
    }

    use super::*;
    use crate::test_server::{TestResponse, TestServer};
    use std::sync::{Arc, Mutex};

    struct Fixture {
        library_id: String,
        revision: u64,
        snapshot: Vec<u8>,
        downloads: usize,
        busy: usize,
    }

    fn sample_snapshot(directory: &Path) -> Vec<u8> {
        let source = directory.join("server.db");
        let provider = SqliteProvider::open(&source).unwrap();
        let backup = directory.join("backup.db");
        provider.backup_library(&backup).unwrap();
        drop(provider);
        std::fs::read(backup).unwrap()
    }

    fn old_unicode_snapshot(directory: &Path) -> Vec<u8> {
        let bytes = sample_snapshot(directory);
        let path = directory.join("old-unicode.db");
        std::fs::write(&path, bytes).unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO audio_files(file_path,path_key,filename,bwf_description_tokenized,metadata_json)
             VALUES('embedded.wav','embedded.wav','embedded.wav','caf é',?1)",
            [r#"{"wav_metadata":{"riff_info":{"ICMT":"café"},"fields":{"description":"café"}}}"#],
        ).unwrap();
        conn.execute(
            "DELETE FROM audiolib_schema_features WHERE feature='tokenization'",
            [],
        )
        .unwrap();
        drop(conn);
        std::fs::read(path).unwrap()
    }

    fn assert_unicode_source(provider: &SqliteProvider) {
        for query in ["cafe", "café", "Cafe\u{301}"] {
            let page = provider
                .search(&audiolib_core::SearchRequest {
                    query: query.into(),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(page.total_count, 1, "{query}");
            assert_eq!(page.results[0].filename, "embedded.wav");
        }
        assert_eq!(
            provider
                .search(&audiolib_core::SearchRequest {
                    query: "caf e".into(),
                    ..Default::default()
                })
                .unwrap()
                .total_count,
            0
        );
    }

    #[test]
    fn validation_upgrades_owned_old_unicode_snapshot_without_sidecars() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("download.db.download");
        std::fs::write(&path, old_unicode_snapshot(directory.path())).unwrap();
        assert!(!SqliteProvider::has_current_search_text(&path).unwrap());
        validate_snapshot(&path).unwrap();
        assert!(SqliteProvider::has_current_search_text(&path).unwrap());
        let provider = SqliteProvider::open_read_only(&path).unwrap();
        assert_unicode_source(&provider);
        drop(provider);
        for suffix in ["-journal", "-wal", "-shm"] {
            assert!(
                !directory
                    .path()
                    .join(format!("download.db.download{suffix}"))
                    .exists()
            );
        }
    }

    #[test]
    fn cancelling_owned_cache_preparation_keeps_original_bytes_and_reader() {
        for stage in [
            CachePreparation::Copying {
                completed: 1,
                total: 1,
            },
            CachePreparation::Validating,
            CachePreparation::Upgrading,
            CachePreparation::Saving,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let cache = RemoteCache::new(&directory.path().join("data"));
            let state = CacheState {
                library_id: "lib-cancel-owned".into(),
                revision: 7,
                schema_version: 3,
                database_file: "library-r7-original.db".into(),
                server_url: "http://offline.test".into(),
                ..Default::default()
            };
            std::fs::create_dir_all(cache.library_dir(&state.library_id)).unwrap();
            let database = cache.database_path(&state);
            std::fs::write(&database, old_unicode_snapshot(directory.path())).unwrap();
            cache.save_state(&state).unwrap();
            let reader = SqliteProvider::open_read_only(&database).unwrap();
            let reader_path = reader.database_path().to_path_buf();
            let state_path = cache.library_dir(&state.library_id).join(STATE_FILE);
            let active_path = cache.root.join(ACTIVE_FILE);
            let before = [
                std::fs::read(&database).unwrap(),
                std::fs::read(&state_path).unwrap(),
                std::fs::read(&active_path).unwrap(),
            ];
            let cancel = AtomicBool::new(false);
            let mut cancelled_at = None;
            let result = cache.prepare_cached_state_with_progress_cancellable(
                &state,
                &mut |current| {
                    // Preparation can take longer than the cancellation budget before
                    // anyone requests cancellation. Exercise that distinction even on
                    // fast machines rather than depending on CI copy/check latency.
                    if stage == CachePreparation::Upgrading && current == CachePreparation::Checking
                    {
                        std::thread::sleep(Duration::from_millis(1100));
                    }
                    let matches = match (stage, current) {
                        (
                            CachePreparation::Copying { .. },
                            CachePreparation::Copying { completed, .. },
                        ) => completed > 0,
                        _ => stage == current,
                    };
                    if matches {
                        cancelled_at.get_or_insert_with(Instant::now);
                        cancel.store(true, Ordering::Release);
                    }
                },
                &cancel,
            );
            assert!(
                matches!(result, Err(RemoteError::Cancelled)),
                "{stage:?}: {result:?}"
            );
            let cancellation_elapsed = cancelled_at
                .expect("the requested cancellation stage must be reached")
                .elapsed();
            assert!(
                cancellation_elapsed < Duration::from_secs(1),
                "{stage:?}: cancellation took {cancellation_elapsed:?}"
            );
            assert_eq!(cache.active().unwrap(), state);
            assert_eq!(
                [
                    std::fs::read(&database).unwrap(),
                    std::fs::read(&state_path).unwrap(),
                    std::fs::read(&active_path).unwrap()
                ],
                before
            );
            assert_eq!(reader.database_path(), reader_path);
            assert_unicode_source(&reader);
            let mut files: Vec<_> = std::fs::read_dir(cache.library_dir(&state.library_id))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            files.sort();
            assert_eq!(files, [STATE_FILE, "library-r7-original.db"], "{stage:?}");
        }
    }

    #[test]
    fn cancelling_downloaded_snapshot_validation_preserves_active_cache() {
        for stage in [CachePreparation::Validating, CachePreparation::Upgrading] {
            let directory = tempfile::tempdir().unwrap();
            let bytes = old_unicode_snapshot(directory.path());
            let server = TestServer::start(move |request| match request.path.as_str() {
                "/api/v1/health" => TestResponse::json(200, r#"{"status":"ok","version":"test"}"#),
                "/api/v1/library" => TestResponse::json(200, &library_json(8)),
                "/api/v1/roots" => TestResponse::json(200, "[]"),
                "/api/v1/library/snapshot" => TestResponse::bytes(200, bytes.clone())
                    .with_header("X-Library-Id", "lib-r")
                    .with_header("X-Library-Revision", "8")
                    .with_header("X-Schema-Version", "3"),
                _ => panic!("unexpected request {}", request.path),
            });
            let cache = RemoteCache::new(&directory.path().join("data"));
            let state = CacheState {
                library_id: "lib-r".into(),
                revision: 7,
                schema_version: 5,
                database_file: "library-r7-original.db".into(),
                server_url: server.url.clone(),
                ..Default::default()
            };
            std::fs::create_dir_all(cache.library_dir(&state.library_id)).unwrap();
            let database = cache.database_path(&state);
            std::fs::write(
                &database,
                std::fs::read(directory.path().join("backup.db")).unwrap(),
            )
            .unwrap();
            cache.save_state(&state).unwrap();
            let reader = SqliteProvider::open_read_only(&database).unwrap();
            let state_path = cache.library_dir(&state.library_id).join(STATE_FILE);
            let active_path = cache.root.join(ACTIVE_FILE);
            let before = [
                std::fs::read(&database).unwrap(),
                std::fs::read(&state_path).unwrap(),
                std::fs::read(&active_path).unwrap(),
            ];
            let cancel = AtomicBool::new(false);
            let client = RemoteClient::new(&server.url, "").unwrap();
            let mut cancelled_at = None;
            let result = sync_library(
                &client,
                &cache,
                &mut |current| {
                    if current == SyncProgress::Preparing(stage) {
                        cancelled_at.get_or_insert_with(Instant::now);
                        cancel.store(true, Ordering::Release);
                    }
                },
                &cancel,
                false,
            );
            assert!(
                matches!(result, Err(RemoteError::Cancelled)),
                "{stage:?}: {result:?}"
            );
            let cancellation_elapsed = cancelled_at
                .expect("the requested cancellation stage must be reached")
                .elapsed();
            assert!(
                cancellation_elapsed < Duration::from_secs(1),
                "{stage:?}: cancellation took {cancellation_elapsed:?}"
            );
            assert_eq!(cache.active().unwrap(), state);
            assert_eq!(
                [
                    std::fs::read(&database).unwrap(),
                    std::fs::read(&state_path).unwrap(),
                    std::fs::read(&active_path).unwrap()
                ],
                before
            );
            assert_eq!(reader.database_path(), database);
            assert_eq!(reader.stats().unwrap().audio_files, 0);
            let mut files: Vec<_> = std::fs::read_dir(cache.library_dir(&state.library_id))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            files.sort();
            assert_eq!(files, [STATE_FILE, "library-r7-original.db"], "{stage:?}");
        }
    }

    #[test]
    fn offline_cache_upgrade_keeps_original_bytes_and_open_provider() {
        let directory = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(directory.path());
        let state = CacheState {
            library_id: "lib-old".into(),
            revision: 7,
            schema_version: 3,
            database_file: "library-r7-old.db".into(),
            server_url: "http://offline.test".into(),
            ..Default::default()
        };
        std::fs::create_dir_all(cache.library_dir(&state.library_id)).unwrap();
        let path = cache.database_path(&state);
        std::fs::write(&path, old_unicode_snapshot(directory.path())).unwrap();
        cache.save_state(&state).unwrap();
        let original = std::fs::read(&path).unwrap();
        let original_provider = SqliteProvider::open_read_only(&path).unwrap();
        let mut events = Vec::new();
        let prepared = cache
            .prepare_cached_state_with_progress(&state, &mut |stage| events.push(stage))
            .unwrap();
        assert_eq!(events.first(), Some(&CachePreparation::Checking));
        assert_eq!(events.last(), Some(&CachePreparation::Saving));
        let copies: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                CachePreparation::Copying { completed, total } if *total > 0 => {
                    Some((*completed, *total))
                }
                _ => None,
            })
            .collect();
        assert!(!copies.is_empty());
        assert!(copies.iter().all(|(done, total)| done <= total));
        assert!(
            copies
                .windows(2)
                .all(|pair| pair[0].0 <= pair[1].0 && pair[0].1 == pair[1].1)
        );
        let (done, total) = copies.last().unwrap();
        assert_eq!(done, total);
        assert!(events.windows(3).any(|stages| stages
            == [
                CachePreparation::Validating,
                CachePreparation::Upgrading,
                CachePreparation::Validating
            ]));
        assert_ne!(prepared.database_file, state.database_file);
        assert_eq!(prepared.revision, state.revision);
        assert_eq!(prepared.server_url, state.server_url);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(original_provider.stats().unwrap().audio_files, 1);
        let upgraded = SqliteProvider::open_read_only(&cache.database_path(&prepared)).unwrap();
        assert_unicode_source(&upgraded);
        assert_eq!(cache.active().unwrap(), prepared);
        events.clear();
        assert_eq!(
            cache
                .prepare_cached_state_with_progress(&prepared, &mut |stage| events.push(stage))
                .unwrap(),
            prepared
        );
        assert_eq!(events, [CachePreparation::Checking]);
        let names: Vec<_> = std::fs::read_dir(cache.library_dir(&state.library_id))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|name| name.contains(".download")
                || name.ends_with("-wal")
                || name.ends_with("-shm")
                || name.ends_with("-journal")),
            "{names:?}"
        );
    }

    #[test]
    fn cache_upgrade_copies_committed_wal_and_reuses_prepared_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(directory.path());
        let state = CacheState {
            library_id: "lib-wal".into(),
            revision: 7,
            schema_version: 3,
            database_file: "library-r7-old.db".into(),
            ..Default::default()
        };
        std::fs::create_dir_all(cache.library_dir(&state.library_id)).unwrap();
        let source = cache.database_path(&state);
        std::fs::write(&source, old_unicode_snapshot(directory.path())).unwrap();
        cache.save_state(&state).unwrap();
        let writer = rusqlite::Connection::open(&source).unwrap();
        writer
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        let main_before = std::fs::read(&source).unwrap();
        writer
            .execute(
                "UPDATE audio_files SET user_notes='committed notes',
             user_notes_tokenized='committed notes' WHERE filename='embedded.wav'",
                [],
            )
            .unwrap();
        let wal = source.with_extension("db-wal");
        let wal_before = std::fs::read(&wal).unwrap();
        assert!(!wal_before.is_empty());

        let prepared = cache.prepare_cached_state(&state).unwrap();
        assert_ne!(prepared.database_file, state.database_file);
        assert_eq!(std::fs::read(&source).unwrap(), main_before);
        assert_eq!(std::fs::read(&wal).unwrap(), wal_before);
        let upgraded_path = cache.database_path(&prepared);
        assert!(SqliteProvider::has_current_search_state(&upgraded_path).unwrap());
        let reader = SqliteProvider::open_read_only(&upgraded_path).unwrap();
        let embedded = reader
            .search(&audiolib_core::SearchRequest {
                query: "committed notes".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(embedded.total_count, 1);
        assert_eq!(embedded.results[0].notes, "committed notes");
        assert_unicode_source(&reader);
        let prepared_bytes = std::fs::read(&upgraded_path).unwrap();
        assert_eq!(cache.prepare_cached_state(&prepared).unwrap(), prepared);
        assert_eq!(std::fs::read(&upgraded_path).unwrap(), prepared_bytes);
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut sidecar = upgraded_path.as_os_str().to_os_string();
            sidecar.push(suffix);
            assert!(!PathBuf::from(sidecar).exists());
        }
        drop(reader);
        drop(writer);
    }

    #[test]
    fn schema_4_cache_upgrade_preserves_original_records_and_reuses_owned_schema_5_copy() {
        let directory = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(directory.path());
        let state = CacheState {
            library_id: "lib-schema-4".into(),
            revision: 279,
            schema_version: 4,
            database_file: "library-r279-schema4.db".into(),
            server_url: "http://offline.test/api/v1".into(),
            ..Default::default()
        };
        std::fs::create_dir_all(cache.library_dir(&state.library_id)).unwrap();
        let source = cache.database_path(&state);
        // The current schema fixture contains valid library tables and triggers. Its
        // missing tokenization marker models an older server snapshot's derived state.
        std::fs::write(&source, old_unicode_snapshot(directory.path())).unwrap();
        let conn = rusqlite::Connection::open(&source).unwrap();
        conn.execute_batch(
            "PRAGMA user_version=4;
             UPDATE audio_files SET user_tags='retained tag',user_tags_tokenized='retained tag',
               user_notes='retained notes',user_notes_tokenized='retained notes',
               user_color='#336699',favorite=1,play_count=12,user_rating=4;",
        )
        .unwrap();
        let id: i64 = conn
            .query_row("SELECT id FROM audio_files", [], |row| row.get(0))
            .unwrap();
        drop(conn);
        cache.save_state(&state).unwrap();
        let original_bytes = std::fs::read(&source).unwrap();
        assert!(!SqliteProvider::has_current_search_state(&source).unwrap());
        let original_reader = SqliteProvider::open_read_only(&source).unwrap();
        let original_record = serde_json::to_value(original_reader.get_audio(id).unwrap()).unwrap();

        let prepared = cache.prepare_cached_state(&state).unwrap();
        assert_ne!(prepared.database_file, state.database_file);
        assert_eq!(prepared.schema_version, 5);
        assert_eq!(prepared.library_id, state.library_id);
        assert_eq!(prepared.revision, state.revision);
        assert_eq!(prepared.server_url, state.server_url);
        assert_eq!(std::fs::read(&source).unwrap(), original_bytes);
        let old = rusqlite::Connection::open_with_flags(
            &source,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(
            old.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            4
        );
        assert_eq!(
            serde_json::to_value(original_reader.get_audio(id).unwrap()).unwrap(),
            original_record
        );

        let owned = cache.database_path(&prepared);
        let reader = SqliteProvider::open_read_only(&owned).unwrap();
        assert_eq!(reader.library_info().unwrap().schema_version, 5);
        assert_eq!(reader.stats().unwrap().audio_files, 1);
        assert_eq!(
            serde_json::to_value(reader.get_audio(id).unwrap()).unwrap(),
            original_record
        );
        let owned_conn = rusqlite::Connection::open_with_flags(
            &owned,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        assert_eq!(
            owned_conn
                .query_row(
                    "SELECT user_rating FROM audio_files WHERE id=?1",
                    [id],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            4,
            "legacy annotations remain stored when the rating UI is removed"
        );
        assert_unicode_source(&reader);
        let owned_bytes = std::fs::read(&owned).unwrap();
        assert_eq!(cache.active().unwrap(), prepared);
        assert_eq!(cache.prepare_cached_state(&prepared).unwrap(), prepared);
        assert_eq!(std::fs::read(&owned).unwrap(), owned_bytes);
        assert_eq!(std::fs::read(&source).unwrap(), original_bytes);
        assert_eq!(
            std::fs::read_dir(cache.library_dir(&state.library_id))
                .unwrap()
                .count(),
            3,
            "retain the original cache, one upgraded cache and cache.json"
        );
    }

    #[test]
    fn invalid_old_cache_upgrade_keeps_previous_state_and_removes_only_staging() {
        let directory = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(directory.path());
        let state = CacheState {
            library_id: "lib-old".into(),
            revision: 7,
            schema_version: 3,
            database_file: "library-r7-old.db".into(),
            ..Default::default()
        };
        std::fs::create_dir_all(cache.library_dir(&state.library_id)).unwrap();
        let path = cache.database_path(&state);
        std::fs::write(&path, old_unicode_snapshot(directory.path())).unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("DROP TRIGGER audio_files_au").unwrap();
        drop(conn);
        cache.save_state(&state).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(cache.prepare_cached_state(&state).is_err());
        assert_eq!(cache.active().unwrap(), state);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(
            std::fs::read_dir(cache.library_dir(&state.library_id))
                .unwrap()
                .count(),
            2
        );
    }

    fn server(fixture: Arc<Mutex<Fixture>>) -> TestServer {
        TestServer::start(move |request| {
            let mut fixture = fixture.lock().unwrap();
            match request.path.as_str() {
                "/api/v1/health" => TestResponse::json(200, r#"{"status":"ok","version":"9.9"}"#),
                "/api/v1/library" => TestResponse::json(
                    200,
                    &format!(
                        r#"{{"library_id":"{}","schema_version":3,"revision":{}}}"#,
                        fixture.library_id, fixture.revision
                    ),
                ),
                "/api/v1/roots" => TestResponse::json(
                    200,
                    r#"[{"name":"SFX","server_path":"/Volumes/SFX","webdav_url":null}]"#,
                ),
                "/api/v1/library/snapshot" if fixture.busy > 0 => {
                    fixture.busy -= 1;
                    TestResponse::json(409, r#"{"error_code":"BUSY","message":"retry shortly"}"#)
                }
                "/api/v1/library/snapshot" => {
                    fixture.downloads += 1;
                    TestResponse::bytes(200, fixture.snapshot.clone())
                        .with_header("X-Library-Id", &fixture.library_id)
                        .with_header("X-Library-Revision", &fixture.revision.to_string())
                        .with_header("X-Schema-Version", "3")
                }
                _ => TestResponse::json(404, "{}"),
            }
        })
    }

    #[test]
    fn snapshot_schema_3_to_5_preserves_source_and_upgrades_only_owned_staging() {
        let directory = tempfile::tempdir().unwrap();
        let bytes = sample_snapshot(directory.path());
        let open_as = |version: i64| {
            let path = directory.path().join(format!("v{version}.db"));
            std::fs::write(&path, &bytes).unwrap();
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", version).unwrap();
            drop(conn);
            path
        };
        for version in [3, 4, 5] {
            let source = open_as(version);
            let source_bytes = std::fs::read(&source).unwrap();
            // A complete derived state can be read directly without modifying the
            // server's original schema, even when the writer schema has advanced.
            let original = SqliteProvider::open_read_only(&source).unwrap();
            assert_eq!(original.library_info().unwrap().schema_version, version);
            drop(original);
            assert_eq!(std::fs::read(&source).unwrap(), source_bytes);

            let staging = directory.path().join(format!("v{version}.db.download"));
            std::fs::write(&staging, &source_bytes).unwrap();
            let info = validate_snapshot(&staging)
                .unwrap_or_else(|error| panic!("v{version} snapshot rejected: {error}"));
            // Validation may migrate this exclusively owned staging file to the
            // current schema and record the actual upgraded version in cache state.
            assert_eq!(info.schema_version, audiolib_db::SCHEMA_VERSION);
            assert!(SqliteProvider::has_current_search_state(&staging).unwrap());
            let provider = SqliteProvider::open_read_only(&staging).unwrap();
            assert_eq!(
                provider.library_info().unwrap().schema_version,
                audiolib_db::SCHEMA_VERSION
            );
            drop(provider);
            let source_conn = rusqlite::Connection::open_with_flags(
                &source,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let stored: i64 = source_conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(stored, version);
            assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
            for suffix in ["-journal", "-wal", "-shm"] {
                let mut sidecar = staging.as_os_str().to_os_string();
                sidecar.push(suffix);
                assert!(!PathBuf::from(sidecar).exists());
            }
        }
        for version in [2, 6] {
            let path = open_as(version);
            let error = validate_snapshot(&path).unwrap_err();
            assert!(
                matches!(&error, RemoteError::Snapshot(text) if text.contains("not supported")),
                "{error:?}"
            );
            assert!(SqliteProvider::open_read_only(&path).is_err());
        }
    }

    #[test]
    fn unsupported_server_schema_rejects_sync_and_force_before_cache_or_download() {
        for version in [2, 6] {
            let directory = tempfile::tempdir().unwrap();
            let cache = RemoteCache::new(&directory.path().join("data"));
            let snapshot = sample_snapshot(directory.path());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let server = {
                let requests = requests.clone();
                TestServer::start(move |request| {
                    requests.lock().unwrap().push(request.path.clone());
                    match request.path.as_str() {
                        "/api/v1/health" => {
                            TestResponse::json(200, r#"{"status":"ok","version":"0.4.0"}"#)
                        }
                        "/api/v1/library" => TestResponse::json(
                            200,
                            &format!(
                                r#"{{"library_id":"lib-1","schema_version":{version},"revision":8}}"#
                            ),
                        ),
                        _ => TestResponse::json(500, "{}"),
                    }
                })
            };
            let state = CacheState {
                library_id: "lib-1".into(),
                revision: 7,
                schema_version: audiolib_db::SCHEMA_VERSION,
                database_file: "library-r7-test.db".into(),
                server_url: server.url.clone(),
                ..Default::default()
            };
            std::fs::create_dir_all(cache.library_dir(&state.library_id)).unwrap();
            let path = cache.database_path(&state);
            std::fs::write(&path, &snapshot).unwrap();
            cache.save_state(&state).unwrap();
            let client = RemoteClient::new(&server.url, "").unwrap();
            for force in [false, true] {
                let error =
                    sync_library(&client, &cache, &mut |_| {}, &AtomicBool::new(false), force)
                        .unwrap_err();
                assert!(
                    matches!(error, RemoteError::Snapshot(ref message) if message.contains(&format!("schema {version}")))
                );
                assert_eq!(cache.active().unwrap(), state);
                assert_eq!(std::fs::read(&path).unwrap(), snapshot);
            }
            assert_eq!(
                requests.lock().unwrap().as_slice(),
                ["/api/v1/health", "/api/v1/library"].repeat(2)
            );
            assert_eq!(
                std::fs::read_dir(cache.library_dir(&state.library_id))
                    .unwrap()
                    .count(),
                2
            );
        }
    }

    #[test]
    fn busy_wait_prefers_retry_after_and_falls_back_to_the_backoff() {
        assert_eq!(busy_wait(2, None), Duration::from_secs(1));
        assert_eq!(busy_wait(1, Some(3)), Duration::from_secs(3));
        assert_eq!(busy_wait(1, Some(600)), MAX_BUSY_WAIT);
        assert_eq!(busy_wait(4, Some(0)), Duration::from_secs(4));
    }

    #[test]
    fn snapshot_insufficient_storage_is_not_retried() {
        let directory = tempfile::tempdir().unwrap();
        let snapshots = Arc::new(Mutex::new(0_usize));
        let server = {
            let snapshots = snapshots.clone();
            TestServer::start(move |request| match request.path.as_str() {
                "/api/v1/health" => TestResponse::json(200, r#"{"status":"ok","version":"9.9"}"#),
                "/api/v1/library" => {
                    TestResponse::json(200, r#"{"library_id":"L","schema_version":3,"revision":1}"#)
                }
                "/api/v1/roots" => TestResponse::json(200, "[]"),
                "/api/v1/library/snapshot" => {
                    *snapshots.lock().unwrap() += 1;
                    TestResponse::json(
                        507,
                        r#"{"error_code":"INSUFFICIENT_STORAGE","message":"disk full"}"#,
                    )
                }
                _ => TestResponse::json(404, "{}"),
            })
        };
        let client = RemoteClient::new(&server.url, "t").unwrap();
        let cache = RemoteCache::new(&directory.path().join("data"));
        let cancel = AtomicBool::new(false);
        let error = match sync_library(&client, &cache, &mut |_| {}, &cancel, false) {
            Ok(_) => panic!("a 507 snapshot must fail"),
            Err(error) => error,
        };
        assert!(
            matches!(error, RemoteError::InsufficientStorage(_)),
            "{error:?}"
        );
        assert_eq!(*snapshots.lock().unwrap(), 1, "507 must not be retried");
    }

    #[test]
    fn busy_window_covers_a_slow_first_snapshot_of_a_large_library() {
        // 90 s until the first 503 BUSY, ~125 s until the snapshot is ready.
        assert!(BUSY_WINDOW >= Duration::from_secs(180));
        assert!(BUSY_WINDOW > Duration::from_secs(125 + 10));
    }

    #[test]
    fn busy_503_and_409_are_retried_on_retry_after_beyond_the_old_four_attempts() {
        let directory = tempfile::tempdir().unwrap();
        let snapshot = sample_snapshot(directory.path());
        let busy = Arc::new(Mutex::new(6_u32));
        let waited = Arc::new(Mutex::new(Vec::<Instant>::new()));
        let server = {
            let (busy, waited) = (busy.clone(), waited.clone());
            TestServer::start(move |request| match request.path.as_str() {
                "/api/v1/health" => TestResponse::json(200, r#"{"status":"ok","version":"9.9"}"#),
                "/api/v1/library" => {
                    TestResponse::json(200, r#"{"library_id":"L","schema_version":3,"revision":1}"#)
                }
                "/api/v1/roots" => TestResponse::json(200, "[]"),
                "/api/v1/library/snapshot" => {
                    let mut left = busy.lock().unwrap();
                    if *left > 0 {
                        *left -= 1;
                        waited.lock().unwrap().push(Instant::now());
                        let status = if *left % 2 == 0 { 503 } else { 409 };
                        return TestResponse::json(
                            status,
                            r#"{"error_code":"BUSY","message":"snapshot is being prepared"}"#,
                        )
                        .with_header("Retry-After", "1");
                    }
                    TestResponse::bytes(200, snapshot.clone())
                        .with_header("X-Library-Id", "L")
                        .with_header("X-Library-Revision", "1")
                        .with_header("X-Schema-Version", "3")
                }
                _ => TestResponse::json(404, "{}"),
            })
        };
        let client = RemoteClient::new(&server.url, "t").unwrap();
        let cache = RemoteCache::new(&directory.path().join("data"));
        let outcome =
            sync_library(&client, &cache, &mut |_| {}, &AtomicBool::new(false), false).unwrap();
        assert!(outcome.downloaded);
        let times = waited.lock().unwrap();
        assert_eq!(times.len(), 6, "six BUSY answers, then success");
        for pair in times.windows(2) {
            assert!(
                pair[1].duration_since(pair[0]) >= Duration::from_millis(900),
                "Retry-After: 1 must be honoured"
            );
        }
    }

    #[test]
    fn busy_backoff_doubles_from_half_a_second() {
        let waits: Vec<u128> = (1..=4_u32)
            .map(|attempt| busy_backoff(attempt).as_millis())
            .collect();
        assert_eq!(waits, [500, 1000, 2000, 4000]);
    }

    #[test]
    fn sync_downloads_only_on_new_revision_or_library_and_rejects_bad_snapshots() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = Arc::new(Mutex::new(Fixture {
            library_id: "lib/one".into(),
            revision: 1,
            snapshot: sample_snapshot(directory.path()),
            downloads: 0,
            busy: 1,
        }));
        let server = server(fixture.clone());
        let client = RemoteClient::new(&server.url, "t").unwrap();
        let cache = RemoteCache::new(&directory.path().join("data"));
        let cancel = AtomicBool::new(false);
        let mut phases = Vec::new();
        let first = sync_library(&client, &cache, &mut |p| phases.push(p), &cancel, false).unwrap();
        assert!(first.downloaded && !first.library_changed && first.previous.is_none());
        assert_eq!(fixture.lock().unwrap().busy, 0, "BUSY must be retried");
        assert!(phases.contains(&SyncProgress::Validating));
        assert_eq!(first.state.server_version, "9.9");
        assert_eq!(first.state.roots[0].server_path, "/Volumes/SFX");
        let db = cache.database_path(&first.state);
        assert!(db.is_file());
        assert!(!cache.library_dir("lib/one").ends_with("lib/one"));
        assert_eq!(cache.active().unwrap(), first.state);
        SqliteProvider::open_read_only(&db).unwrap();

        phases.clear();
        let second = sync_library(
            &client,
            &cache,
            &mut |stage| phases.push(stage),
            &cancel,
            false,
        )
        .unwrap();
        assert!(!second.downloaded);
        assert_eq!(second.state.database_file, first.state.database_file);
        assert_eq!(
            phases,
            [
                SyncProgress::Checking,
                SyncProgress::Preparing(CachePreparation::Checking),
                SyncProgress::Saving
            ]
        );
        assert_eq!(fixture.lock().unwrap().downloads, 1);

        fixture.lock().unwrap().revision = 2;
        std::thread::sleep(std::time::Duration::from_millis(5));
        let third = sync_library(&client, &cache, &mut |_| {}, &cancel, false).unwrap();
        assert!(third.downloaded);
        assert_eq!(third.state.revision, 2);
        assert_ne!(third.state.database_file, first.state.database_file);

        // A corrupt snapshot leaves the current cache active and removes the staging file.
        {
            let mut fixture = fixture.lock().unwrap();
            fixture.revision = 3;
            fixture.snapshot = vec![1u8; 8192];
        }
        let error = sync_library(&client, &cache, &mut |_| {}, &cancel, false).unwrap_err();
        assert!(matches!(error, RemoteError::Snapshot(_)), "{error:?}");
        assert_eq!(cache.active().unwrap().revision, 2);
        let leftovers = std::fs::read_dir(cache.library_dir("lib/one"))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".download"))
            .count();
        assert_eq!(leftovers, 0);

        // A different library identity is reported so the GUI can warn and reset; the
        // replaced library's snapshots and playback audio cache stay until the swap.
        let old_audio = cache.audio_cache_dir("lib/one").join("1-abc.wav");
        std::fs::create_dir_all(old_audio.parent().unwrap()).unwrap();
        std::fs::write(&old_audio, b"RIFF").unwrap();
        {
            let mut fixture = fixture.lock().unwrap();
            fixture.library_id = "lib-two".into();
            fixture.snapshot = sample_snapshot(&directory.path().join("again"));
        }
        let changed = sync_library(&client, &cache, &mut |_| {}, &cancel, false).unwrap();
        assert!(changed.library_changed && changed.downloaded);
        assert_eq!(changed.previous.unwrap().library_id, "lib/one");
        assert_eq!(cache.active().unwrap().library_id, "lib-two");
        assert_eq!(changed.state.revision, 3);
        assert!(
            old_audio.is_file(),
            "the old library's cache must survive the sync"
        );
        cache.cleanup_after_swap(&changed.state);
        assert!(!cache.library_dir("lib/one").exists());
        assert!(cache.state("lib/one").is_none());
    }

    #[test]
    fn sync_keeps_the_open_snapshot_and_stale_cleanup_never_deletes_the_new_one() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = Arc::new(Mutex::new(Fixture {
            library_id: "lib-a".into(),
            revision: 1,
            snapshot: sample_snapshot(directory.path()),
            downloads: 0,
            busy: 0,
        }));
        let server = server(fixture.clone());
        let client = RemoteClient::new(&server.url, "").unwrap();
        let cache = RemoteCache::new(&directory.path().join("data"));
        let cancel = AtomicBool::new(false);
        let opened = sync_library(&client, &cache, &mut |_| {}, &cancel, false)
            .unwrap()
            .state;
        let audio = cache.audio_cache_dir("lib-a").join("1-x.wav");
        std::fs::create_dir_all(audio.parent().unwrap()).unwrap();
        std::fs::write(&audio, b"RIFF").unwrap();

        // Same library, new revision: the open snapshot survives the sync, and a poller
        // still holding the old state keeps the freshly installed file too.
        fixture.lock().unwrap().revision = 2;
        std::thread::sleep(Duration::from_millis(5));
        let newer = sync_library(&client, &cache, &mut |_| {}, &cancel, false)
            .unwrap()
            .state;
        assert!(cache.database_path(&opened).is_file());
        cache.cleanup_after_swap(&opened);
        assert!(cache.database_path(&opened).is_file());
        assert!(cache.database_path(&newer).is_file());
        cache.cleanup_after_swap(&newer);
        assert!(!cache.database_path(&opened).exists());
        assert!(cache.database_path(&newer).is_file());

        // Another library: until the swap, cleanup with the stale opened state and a
        // direct discard both leave the new library and the old one (and its audio).
        {
            let mut fixture = fixture.lock().unwrap();
            fixture.library_id = "lib-b".into();
            fixture.snapshot = sample_snapshot(&directory.path().join("b"));
        }
        let replacement = sync_library(&client, &cache, &mut |_| {}, &cancel, false)
            .unwrap()
            .state;
        cache.cleanup_after_swap(&newer);
        assert!(cache.discard_replaced(&newer).is_empty());
        assert!(cache.database_path(&replacement).is_file());
        assert!(cache.database_path(&newer).is_file());
        assert!(audio.is_file());
        cache.cleanup_after_swap(&replacement);
        assert!(!cache.library_dir("lib-a").exists());
        assert!(cache.database_path(&replacement).is_file());
    }

    #[test]
    fn startup_staging_prune_keeps_snapshots_and_fresh_staging() {
        let directory = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(directory.path());
        let dir = cache.library_dir("lib");
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "library-r1-1.db",
            "library-r2-2.db.download",
            "library-r3-3.db.download",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let old = std::time::SystemTime::now() - STALE_STAGING_AGE - Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(dir.join("library-r2-2.db.download"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        cache.prune_stale_staging("lib");
        assert!(dir.join("library-r1-1.db").exists());
        assert!(!dir.join("library-r2-2.db.download").exists());
        assert!(dir.join("library-r3-3.db.download").exists());
        cache.prune_stale_staging("missing-library");
    }

    #[test]
    fn prune_removes_only_stale_staging_files() {
        let directory = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(directory.path());
        let dir = cache.library_dir("lib");
        std::fs::create_dir_all(&dir).unwrap();
        let names = [
            "library-r1-1.db",
            "library-r2-2.db",
            "library-r3-3.db.download",
            "library-r3-3.db.download.part-77",
            "library-r0-0.db.part-12",
        ];
        for name in names {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        cache.prune("lib", &["library-r2-2.db"]);
        let left = |name: &str| dir.join(name).exists();
        assert!(!left("library-r1-1.db") && left("library-r2-2.db"));
        // Fresh staging files may belong to a running sync.
        assert!(left("library-r3-3.db.download") && left("library-r3-3.db.download.part-77"));
        assert!(left("library-r0-0.db.part-12"));
        let old = std::time::SystemTime::now() - STALE_STAGING_AGE - Duration::from_secs(60);
        for name in &names[2..] {
            std::fs::File::options()
                .write(true)
                .open(dir.join(name))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        cache.prune("lib", &["library-r2-2.db"]);
        for name in &names[2..] {
            assert!(!left(name), "{name}");
        }
        assert!(left("library-r2-2.db"));
    }

    #[test]
    fn discard_replaced_removes_only_this_servers_old_libraries() {
        let directory = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(directory.path());
        let make = |id: &str, server: &str| {
            let state = CacheState {
                library_id: id.into(),
                revision: 4,
                database_file: "library-r4-1.db".into(),
                server_url: server.into(),
                ..Default::default()
            };
            let dir = cache.library_dir(id);
            std::fs::create_dir_all(dir.join("audio-cache")).unwrap();
            drop(SqliteProvider::open(&dir.join(&state.database_file)).unwrap());
            std::fs::write(dir.join("audio-cache").join("7-x.wav"), b"RIFF").unwrap();
            std::fs::write(dir.join(STATE_FILE), serde_json::to_vec(&state).unwrap()).unwrap();
            state
        };
        let old = make("old", "http://nas:8790/api/v1");
        let other_server = make("elsewhere", "http://other:8790/api/v1");
        let active = make("new", "http://nas:8790/api/v1");
        // The old snapshot is still open (Windows cannot delete it): kept for a retry.
        let open = rusqlite::Connection::open(cache.database_path(&old)).unwrap();
        open.query_row("SELECT count(*) FROM audio_files", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        let removed = cache.discard_replaced(&active);
        if cfg!(windows) {
            assert!(removed.is_empty());
            assert!(cache.library_dir("old").join(STATE_FILE).is_file());
            assert!(!cache.audio_cache_dir("old").join("7-x.wav").exists());
        }
        drop(open);
        cache.discard_replaced(&active);
        assert!(!cache.library_dir("old").exists());
        assert!(cache.state(&other_server.library_id).is_some());
        assert!(cache.audio_cache_dir("elsewhere").join("7-x.wav").is_file());
        assert!(cache.state(&active.library_id).is_some());
    }

    #[test]
    fn validation_accepts_a_good_snapshot_and_rejects_a_dropped_trigger() {
        let directory = tempfile::tempdir().unwrap();
        let good = directory.path().join("good.db");
        std::fs::write(&good, sample_snapshot(directory.path())).unwrap();
        validate_snapshot(&good).unwrap();
        // Nothing is left beside it (no journal, WAL or copy) and the file stays a
        // rollback-journal database that a read-only provider can open.
        let names: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|name| name.starts_with("good.db-")),
            "{names:?}"
        );
        let mode: String = rusqlite::Connection::open(&good)
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "delete");
        let bad = directory.path().join("bad.db");
        std::fs::copy(&good, &bad).unwrap();
        let name: String = rusqlite::Connection::open(&bad)
            .unwrap()
            .query_row(
                "SELECT name FROM sqlite_schema WHERE type='trigger' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        rusqlite::Connection::open(&bad)
            .unwrap()
            .execute_batch(&format!("DROP TRIGGER \"{name}\""))
            .unwrap();
        let error = validate_snapshot(&bad).unwrap_err();
        assert!(matches!(error, RemoteError::Snapshot(_)), "{error:?}");
    }

    #[test]
    fn placeholder_is_empty_and_read_only_openable() {
        let directory = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(directory.path());
        let path = cache.placeholder_database().unwrap();
        let provider = SqliteProvider::open_read_only(&path).unwrap();
        assert_eq!(provider.stats().unwrap().audio_files, 0);
        assert_eq!(cache.placeholder_database().unwrap(), path);
        assert!(cache.active().is_none());
    }

    #[test]
    fn old_audition_caches_are_removed_and_nothing_else() {
        let directory = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(directory.path());
        let audio = cache.audio_cache_dir("lib-a");
        std::fs::create_dir_all(&audio).unwrap();
        std::fs::write(audio.join("1-x.wav"), b"RIFF").unwrap();
        std::fs::write(audio.join("1-x.wav.etag"), b"\"v1\"").unwrap();
        std::fs::write(audio.join("2-y.wav.part-1-2-3"), b"").unwrap();
        let snapshot = cache.library_dir("lib-a").join("library-r1-1.db");
        std::fs::write(&snapshot, b"db").unwrap();
        let drag = directory
            .path()
            .join("cache")
            .join("lib-a")
            .join("7-0123456789abcdef");
        std::fs::create_dir_all(&drag).unwrap();
        std::fs::write(drag.join("x.wav"), b"RIFF").unwrap();
        assert_eq!(cache.remove_audition_caches(), 1);
        assert!(!audio.exists());
        assert!(snapshot.is_file());
        assert!(drag.join("x.wav").is_file());
        assert_eq!(cache.remove_audition_caches(), 0);
    }

    fn library_json(revision: u64) -> String {
        format!(r#"{{"library_id":"lib-r","schema_version":3,"revision":{revision}}}"#)
    }

    #[test]
    fn cancelling_each_http_sync_phase_keeps_existing_cache_and_removes_staging() {
        for (path, stalled_body) in [
            ("/api/v1/health", false),
            ("/api/v1/library", false),
            ("/api/v1/roots", false),
            ("/api/v1/library/snapshot", false),
            ("/api/v1/library/snapshot", true),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let bytes = sample_snapshot(directory.path());
            let enabled = Arc::new(AtomicBool::new(false));
            let (requested, waiting) = std::sync::mpsc::sync_channel(1);
            let server = {
                let enabled = enabled.clone();
                TestServer::start(move |request| {
                    let response = match request.path.as_str() {
                        "/api/v1/health" => {
                            TestResponse::json(200, r#"{"status":"ok","version":"test"}"#)
                        }
                        "/api/v1/library" => TestResponse::json(200, &library_json(1)),
                        "/api/v1/roots" => TestResponse::json(200, "[]"),
                        "/api/v1/library/snapshot" => TestResponse::bytes(200, bytes.clone())
                            .with_header("X-Library-Id", "lib-r")
                            .with_header("X-Library-Revision", "1")
                            .with_header("X-Schema-Version", "3"),
                        _ => panic!("unexpected request {}", request.path),
                    };
                    if enabled.load(Ordering::Acquire) && request.path == path {
                        let _ = requested.send(());
                        if stalled_body {
                            response.slow(1024, Duration::from_secs(2))
                        } else {
                            response.delayed(Duration::from_secs(2))
                        }
                    } else {
                        response
                    }
                })
            };
            let client = RemoteClient::new(&server.url, "").unwrap();
            let cache = RemoteCache::new(&directory.path().join("data"));
            let first =
                sync_library(&client, &cache, &mut |_| {}, &AtomicBool::new(false), false).unwrap();
            let state_path = cache.library_dir("lib-r").join(STATE_FILE);
            let active_path = cache.root.join(ACTIVE_FILE);
            let database = cache.database_path(&first.state);
            let before = (
                std::fs::read(&state_path).unwrap(),
                std::fs::read(&active_path).unwrap(),
                std::fs::read(&database).unwrap(),
            );
            enabled.store(true, Ordering::Release);
            let cancel = Arc::new(AtomicBool::new(false));
            let flag = cancel.clone();
            let cancelled = std::thread::spawn(move || {
                waiting.recv_timeout(Duration::from_secs(3)).unwrap();
                std::thread::sleep(Duration::from_millis(100));
                flag.store(true, Ordering::Release);
                Instant::now()
            });
            let result = sync_library(&client, &cache, &mut |_| {}, &cancel, true);
            let returned = Instant::now();
            let cancelled_at = cancelled.join().unwrap();
            assert!(
                matches!(result, Err(RemoteError::Cancelled)),
                "{path}: {result:?}"
            );
            assert!(
                returned.saturating_duration_since(cancelled_at) < Duration::from_secs(1),
                "{path}, stalled_body={stalled_body}, cancellation elapsed={:?}",
                returned.saturating_duration_since(cancelled_at)
            );
            assert_eq!(cache.active().unwrap(), first.state, "{path}");
            assert_eq!(std::fs::read(&state_path).unwrap(), before.0);
            assert_eq!(std::fs::read(&active_path).unwrap(), before.1);
            assert_eq!(std::fs::read(&database).unwrap(), before.2);
            let mut files = std::fs::read_dir(cache.library_dir("lib-r"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            files.sort();
            let mut expected = vec![STATE_FILE.to_owned(), first.state.database_file];
            expected.sort();
            assert_eq!(files, expected, "cancel left staging artifacts at {path}");
        }
    }

    #[test]
    fn a_snapshot_timeout_or_lost_connection_is_retried_a_few_times() {
        let directory = tempfile::tempdir().unwrap();
        let snapshot = sample_snapshot(directory.path());
        let state = Arc::new(Mutex::new((0usize, 2usize))); // (snapshot requests, failures left)
        let server = {
            let state = state.clone();
            TestServer::start(move |request| match request.path.as_str() {
                "/api/v1/health" => TestResponse::json(200, r#"{"status":"ok","version":"9.9"}"#),
                "/api/v1/library" => TestResponse::json(200, &library_json(1)),
                "/api/v1/roots" => TestResponse::json(200, "[]"),
                "/api/v1/library/snapshot" => {
                    let mut state = state.lock().unwrap();
                    state.0 += 1;
                    if state.1 > 0 {
                        state.1 -= 1;
                        TestResponse::json(
                            408,
                            r#"{"error_code":"TIMEOUT","message":"request took longer than 30 s"}"#,
                        )
                    } else {
                        TestResponse::bytes(200, snapshot.clone())
                            .with_header("X-Library-Id", "lib-r")
                            .with_header("X-Library-Revision", "1")
                            .with_header("X-Schema-Version", "3")
                    }
                }
                _ => TestResponse::json(404, "{}"),
            })
        };
        let client = RemoteClient::new(&server.url, "t").unwrap();
        let cache = RemoteCache::new(&directory.path().join("data"));
        let cancel = AtomicBool::new(false);
        let outcome = sync_library(&client, &cache, &mut |_| {}, &cancel, false).unwrap();
        assert!(outcome.downloaded);
        assert_eq!(
            state.lock().unwrap().0,
            3,
            "two timeouts, then the download"
        );

        // A server that keeps timing out is given up on after the bounded retries, with the
        // original (slow, not offline) error, and the cache stays as it was.
        *state.lock().unwrap() = (0, usize::MAX);
        let error = sync_library(&client, &cache, &mut |_| {}, &cancel, true).unwrap_err();
        assert!(error.is_slow() && !error.is_unreachable(), "{error:?}");
        assert_eq!(state.lock().unwrap().0 as u32, TRANSIENT_RETRIES + 1);
        assert_eq!(cache.active().unwrap(), outcome.state);

        // A cancel during the wait stops the retries at once.
        *state.lock().unwrap() = (0, usize::MAX);
        let cancelled = AtomicBool::new(true);
        let error = sync_library(&client, &cache, &mut |_| {}, &cancelled, true).unwrap_err();
        assert!(matches!(error, RemoteError::Cancelled), "{error:?}");
    }

    #[test]
    fn roots_remain_available_while_slow_but_are_persisted_only_after_identity_confirmation() {
        let directory = tempfile::tempdir().unwrap();
        let snapshot = sample_snapshot(directory.path());
        // (library answers slowly, roots body)
        let state = Arc::new(Mutex::new((
            false,
            r#"[{"name":"SFX","server_path":"/Volumes/SFX","webdav_url":null}]"#.to_owned(),
        )));
        let server = {
            let state = state.clone();
            TestServer::start(move |request| {
                let state = state.lock().unwrap();
                match request.path.as_str() {
                    "/api/v1/health" => {
                        TestResponse::json(200, r#"{"status":"ok","version":"9.9"}"#)
                    }
                    "/api/v1/library" if state.0 => TestResponse::json(
                        408,
                        r#"{"error_code":"TIMEOUT","message":"request took longer than 30 s"}"#,
                    ),
                    "/api/v1/library" => TestResponse::json(200, &library_json(1)),
                    "/api/v1/roots" => TestResponse::json(200, &state.1),
                    "/api/v1/library/snapshot" => TestResponse::bytes(200, snapshot.clone())
                        .with_header("X-Library-Id", "lib-r")
                        .with_header("X-Library-Revision", "1")
                        .with_header("X-Schema-Version", "3"),
                    _ => TestResponse::json(404, "{}"),
                }
            })
        };
        let client = RemoteClient::new(&server.url, "t").unwrap();
        let cache = RemoteCache::new(&directory.path().join("data"));
        let cancel = AtomicBool::new(false);
        let first = sync_library(&client, &cache, &mut |_| {}, &cancel, false).unwrap();
        assert_eq!(first.state.roots[0].name, "SFX");

        // A slow library check still allows roots to be fetched, without changing
        // the saved mapping before this library's identity is confirmed again.
        *state.lock().unwrap() = (
            true,
            r#"[{"name":"SFX","server_path":"/Volumes/SFX","webdav_url":null},{"name":"ExampleLibrary","server_path":"/Volumes/ExampleLibrary","webdav_url":null}]"#
                .to_owned(),
        );
        let error = sync_library(&client, &cache, &mut |_| {}, &cancel, false).unwrap_err();
        assert!(error.is_slow(), "{error:?}");
        let roots = refresh_cached_roots(&client, &cache, None).unwrap();
        let names: Vec<&str> = roots.iter().map(|root| root.name.as_str()).collect();
        assert_eq!(names, ["SFX", "ExampleLibrary"]);
        assert_eq!(cache.active().unwrap(), first.state);

        let mut unmatching = RemoteLibrary {
            library_id: "replacement-library".into(),
            schema_version: audiolib_db::SCHEMA_VERSION,
            ..Default::default()
        };
        refresh_cached_roots(&client, &cache, Some(&unmatching)).unwrap();
        assert_eq!(cache.active().unwrap(), first.state);
        unmatching.library_id = first.state.library_id.clone();
        unmatching.schema_version = MAX_SNAPSHOT_SCHEMA + 1;
        refresh_cached_roots(&client, &cache, Some(&unmatching)).unwrap();
        assert_eq!(cache.active().unwrap(), first.state);

        // Once /library confirms the matching identity and endpoint, only roots
        // change; the snapshot file and revision remain untouched.
        state.lock().unwrap().0 = false;
        let library = client.library().unwrap();
        let database = cache.database_path(&first.state);
        let snapshot_before = std::fs::read(&database).unwrap();
        let refreshed = refresh_cached_roots(&client, &cache, Some(&library)).unwrap();
        let mut expected = first.state;
        expected.roots = refreshed;
        assert_eq!(cache.active().unwrap(), expected);
        assert_eq!(std::fs::read(database).unwrap(), snapshot_before);
    }

    #[test]
    fn another_servers_timeout_cannot_change_cached_roots_or_snapshot_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let cache = RemoteCache::new(&directory.path().join("data"));
        let cached = CacheState {
            library_id: "library-A".into(),
            revision: 7,
            schema_version: audiolib_db::SCHEMA_VERSION,
            database_file: "library-r7.db".into(),
            server_url: "http://server-A.invalid/api/v1".into(),
            roots: vec![RemoteRoot {
                name: "A_ROOT".into(),
                server_path: "/A".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        std::fs::create_dir_all(cache.library_dir(&cached.library_id)).unwrap();
        let database = cache.database_path(&cached);
        std::fs::write(&database, sample_snapshot(directory.path())).unwrap();
        cache.save_state(&cached).unwrap();
        let state_path = cache.library_dir(&cached.library_id).join(STATE_FILE);
        let active_path = cache.root.join(ACTIVE_FILE);
        let before = (
            std::fs::read(&state_path).unwrap(),
            std::fs::read(&active_path).unwrap(),
            std::fs::read(&database).unwrap(),
        );
        let server = TestServer::start(|request| match request.path.as_str() {
            "/api/v1/health" => TestResponse::json(200, r#"{"status":"ok"}"#),
            "/api/v1/library" => TestResponse::json(
                408,
                r#"{"error_code":"TIMEOUT","message":"library B timed out"}"#,
            ),
            "/api/v1/roots" => TestResponse::json(200, r#"[{"name":"B_ROOT","server_path":"/B"}]"#),
            _ => panic!("unexpected request: {}", request.path),
        });
        let client = RemoteClient::new(&server.url, "fixture").unwrap();
        let error =
            sync_library(&client, &cache, &mut |_| {}, &AtomicBool::new(false), false).unwrap_err();
        assert!(error.is_slow(), "{error:?}");
        let roots = refresh_cached_roots(&client, &cache, None).unwrap();
        assert_eq!(roots[0].name, "B_ROOT");
        // Even a matching library ID cannot authorize updating another endpoint.
        let same_id = RemoteLibrary {
            library_id: cached.library_id.clone(),
            schema_version: audiolib_db::SCHEMA_VERSION,
            ..Default::default()
        };
        refresh_cached_roots(&client, &cache, Some(&same_id)).unwrap();
        assert_eq!(cache.active().unwrap(), cached);
        assert_eq!(std::fs::read(state_path).unwrap(), before.0);
        assert_eq!(std::fs::read(active_path).unwrap(), before.1);
        assert_eq!(std::fs::read(database).unwrap(), before.2);
    }

    #[test]
    fn roots_endpoint_comparison_normalizes_only_equivalent_api_addresses() {
        assert!(same_api_endpoint(
            "https://Example.test/Library/api/v1/",
            "https://example.test:443/Library"
        ));
        assert!(same_api_endpoint(
            "http://example.test:80/api/v1",
            "http://EXAMPLE.test"
        ));
        for different in [
            "http://example.test/Library",
            "https://other.test/Library",
            "https://example.test:444/Library",
            "https://example.test/library",
            "https://example.test/Other",
            "not an address",
        ] {
            assert!(
                !same_api_endpoint("https://example.test/Library", different),
                "{different}"
            );
        }
    }
}
