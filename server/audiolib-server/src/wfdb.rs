//! `waveforms.db`: list thumbnail waveforms in the 8-bit encoding (doc/api/README.md §9.4).
//!
//! A separate SQLite file next to the list shard cache (never the library database):
//! one row per record, `blob` exactly 256 bytes (128 × int8 min/max, zero padded).
//! WAL, `synchronous=NORMAL`, plain `INTEGER PRIMARY KEY`; rows are UPSERTed in
//! transactions of a few hundred.

use audiolib_core::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

/// `encoding` of the 8-bit list waveforms.
pub const I8_ENCODING: &str = "i8-minmax-base64";
/// Bytes of every `blob`.
pub const BLOB_BYTES: usize = 256;
/// `meta.schema_version` of `waveforms.db` and its bundles.
pub const SCHEMA_VERSION: u32 = 1;
/// File name inside the list cache directory.
pub const FILE_NAME: &str = "waveforms.db";
/// Rows queued before a write transaction is made at once.
pub const BATCH_ROWS: usize = 256;
/// Queued rows are written after at most this long.
const BATCH_AFTER: Duration = Duration::from_secs(2);

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS list_waveforms (
  file_id   INTEGER PRIMARY KEY,
  etag      TEXT    NOT NULL,
  src_mtime INTEGER NOT NULL,
  src_size  INTEGER NOT NULL,
  encoding  TEXT    NOT NULL,
  n         INTEGER NOT NULL,
  blob      BLOB    NOT NULL
);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// `round(i16 * 127 / 32767)`, half away from zero (`f64::round`); never -128.
pub fn i16_to_i8(value: i16) -> i8 {
    let value = value.max(-32767);
    (f64::from(value) * 127.0 / 32767.0).round() as i8
}

/// Interleaved int16 min/max values to int8 bytes (`2 n` of them).
pub fn to_i8_bytes(values: &[i16]) -> Vec<u8> {
    values.iter().map(|v| i16_to_i8(*v) as u8).collect()
}

/// Stored blob: the int8 bytes zero padded to [`BLOB_BYTES`].
pub fn to_blob(values: &[i16]) -> Vec<u8> {
    let mut blob = to_i8_bytes(values);
    blob.truncate(BLOB_BYTES);
    blob.resize(BLOB_BYTES, 0);
    blob
}

/// base64 length of `n` columns: `4 * ceil(2n / 3)`.
pub fn base64_len(n: usize) -> usize {
    4 * (2 * n).div_ceil(3)
}

/// One row of `list_waveforms`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub file_id: i64,
    pub etag: String,
    pub src_mtime: i64,
    pub src_size: i64,
    pub n: usize,
    pub blob: Vec<u8>,
}

/// Open, configured connection plus the queue of unwritten rows.
pub struct WaveformsDb {
    path: PathBuf,
    /// Opened (and the file created) on first write, so an unused server leaves no file.
    conn: Mutex<Option<Connection>>,
    queue: Mutex<(Vec<Row>, Option<Instant>)>,
}

fn sql(error: rusqlite::Error) -> Error {
    Error::Io(std::io::Error::other(format!("waveforms.db: {error}")))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Pragmas and schema of a writable `waveforms.db` connection.
pub fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.execute_batch(SCHEMA)?;
    conn.execute(
        "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

/// UPSERT `rows` in one transaction.
pub fn upsert_rows(conn: &mut Connection, rows: &[Row]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut statement = tx.prepare_cached(
            "INSERT INTO list_waveforms(file_id, etag, src_mtime, src_size, encoding, n, blob)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(file_id) DO UPDATE SET etag = excluded.etag,
               src_mtime = excluded.src_mtime, src_size = excluded.src_size,
               encoding = excluded.encoding, n = excluded.n, blob = excluded.blob",
        )?;
        for row in rows {
            debug_assert_eq!(row.blob.len(), BLOB_BYTES);
            statement.execute(params![
                row.file_id,
                row.etag,
                row.src_mtime,
                row.src_size,
                I8_ENCODING,
                row.n as i64,
                row.blob
            ])?;
        }
    }
    tx.commit()
}

impl WaveformsDb {
    /// `dir/waveforms.db`; opened lazily (an existing file is opened on first read,
    /// a missing one is created on first write).
    pub fn open(dir: &Path) -> Result<Self> {
        // No bundle can be in progress yet: drop leftovers of a crash or kill -9.
        sweep_stale_bundles(dir, Duration::ZERO);
        Ok(Self {
            path: dir.join(FILE_NAME),
            conn: Mutex::new(None),
            queue: Mutex::new((Vec::new(), None)),
        })
    }

    /// The connection; `create` makes the file when it does not exist yet.
    fn connection(&self, create: bool) -> Result<Option<MutexGuard<'_, Option<Connection>>>> {
        let mut guard = lock(&self.conn);
        if guard.is_none() {
            if !create && !self.path.exists() {
                return Ok(None);
            }
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let conn = Connection::open(&self.path).map_err(sql)?;
            configure(&conn).map_err(sql)?;
            *guard = Some(conn);
        }
        Ok(Some(guard))
    }

    /// Path of the database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stored entity tag of `file_id` (written rows and queued ones).
    pub fn etag(&self, file_id: i64) -> Option<String> {
        if let Some(row) = lock(&self.queue)
            .0
            .iter()
            .rev()
            .find(|r| r.file_id == file_id)
        {
            return Some(row.etag.clone());
        }
        let guard = self.connection(false).ok().flatten()?;
        guard
            .as_ref()?
            .query_row(
                "SELECT etag FROM list_waveforms WHERE file_id = ?1",
                [file_id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Stored row of `file_id` (written rows only).
    pub fn get(&self, file_id: i64) -> Option<Row> {
        let guard = self.connection(false).ok().flatten()?;
        guard
            .as_ref()?
            .query_row(
                "SELECT file_id, etag, src_mtime, src_size, n, blob FROM list_waveforms
                 WHERE file_id = ?1",
                [file_id],
                |row| {
                    Ok(Row {
                        file_id: row.get(0)?,
                        etag: row.get(1)?,
                        src_mtime: row.get(2)?,
                        src_size: row.get(3)?,
                        n: row.get::<_, i64>(4)? as usize,
                        blob: row.get(5)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Queue a row; writes a transaction once [`BATCH_ROWS`] are queued.
    pub fn queue(&self, row: Row) {
        let full = {
            let mut queue = lock(&self.queue);
            queue.0.push(row);
            queue.1.get_or_insert_with(Instant::now);
            queue.0.len() >= BATCH_ROWS
        };
        if full {
            self.flush();
        }
    }

    /// Write the queue when it is older than 2 s.
    pub fn flush_due(&self) {
        let due = lock(&self.queue)
            .1
            .is_some_and(|since| since.elapsed() >= BATCH_AFTER);
        if due {
            self.flush();
        }
    }

    /// Write every queued row in one transaction; a failure only costs a recomputation.
    pub fn flush(&self) {
        let rows = {
            let mut queue = lock(&self.queue);
            queue.1 = None;
            std::mem::take(&mut queue.0)
        };
        if rows.is_empty() {
            return;
        }
        let result = self.connection(true).and_then(|guard| {
            let mut guard = guard.expect("created");
            upsert_rows(guard.as_mut().expect("open"), &rows).map_err(sql)
        });
        if let Err(error) = result {
            tracing::warn!(%error, rows = rows.len(), "Cannot write waveforms.db");
        }
    }

    /// Blocking. Delete every row (and drop queued ones) for a `rebuild` job (§9.5.1).
    pub fn clear(&self) -> Result<()> {
        lock(&self.queue).0.clear();
        lock(&self.queue).1 = None;
        let Some(mut guard) = self.connection(false)? else {
            return Ok(());
        };
        let Some(conn) = guard.as_mut() else {
            return Ok(());
        };
        conn.execute("DELETE FROM list_waveforms", [])
            .map_err(sql)?;
        Ok(())
    }

    /// Delete rows whose id is not in `keep`.
    pub fn retain(&self, keep: impl Fn(i64) -> bool) {
        self.flush();
        let Ok(Some(mut guard)) = self.connection(false) else {
            return;
        };
        let Some(conn) = guard.as_mut() else { return };
        let ids: Vec<i64> = match conn
            .prepare("SELECT file_id FROM list_waveforms")
            .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
        {
            Ok(ids) => ids,
            Err(_) => return,
        };
        let gone: Vec<i64> = ids.into_iter().filter(|id| !keep(*id)).collect();
        if gone.is_empty() {
            return;
        }
        let result = conn.transaction().and_then(|tx| {
            for id in &gone {
                tx.execute("DELETE FROM list_waveforms WHERE file_id = ?1", [id])?;
            }
            tx.commit()
        });
        if let Err(error) = result {
            tracing::warn!(%error, "Cannot prune waveforms.db");
        }
    }

    /// Rows stored.
    pub fn count(&self) -> u64 {
        let Ok(Some(guard)) = self.connection(false) else {
            return 0;
        };
        guard
            .as_ref()
            .map_or(Ok(0), |conn| {
                conn.query_row("SELECT COUNT(*) FROM list_waveforms", [], |r| {
                    r.get::<_, i64>(0)
                })
            })
            .unwrap_or(0) as u64
    }

    /// Blocking. Write a read-only snapshot to `dest` (`VACUUM INTO` on a separate
    /// read-only connection, so writers and `POST /waveforms` lookups are not blocked),
    /// then stamp its `meta` with `generated_at`, `library_id`, `revision` and
    /// `encoding`. Returns the bundle's entity tag (SHA-256 of the finished file).
    pub fn bundle(&self, dest: &Path, library_id: &str, revision: u64) -> Result<String> {
        self.flush();
        // Make sure the file and its WAL exist before the read-only connection opens it.
        drop(self.connection(true)?);
        let generated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        {
            let reader = Connection::open_with_flags(
                &self.path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(sql)?;
            reader.busy_timeout(Duration::from_secs(5)).map_err(sql)?;
            reader
                .execute(
                    "VACUUM INTO ?1",
                    [dest.to_str().ok_or_else(|| {
                        Error::Io(std::io::Error::other("bundle path is not valid UTF-8"))
                    })?],
                )
                .map_err(sql)?;
        }
        let conn = Connection::open(dest).map_err(sql)?;
        conn.pragma_update(None, "journal_mode", "DELETE")
            .map_err(sql)?;
        for (key, value) in [
            ("generated_at", generated_at.to_string()),
            ("library_id", library_id.to_string()),
            ("revision", revision.to_string()),
            ("schema_version", SCHEMA_VERSION.to_string()),
            ("encoding", I8_ENCODING.to_string()),
        ] {
            conn.execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value.as_str()],
            )
            .map_err(sql)?;
        }
        drop(conn);
        file_etag(dest)
    }
}

/// Bundle entity tag: `"wfb1-<first 16 bytes of SHA-256 of the file, hex>"`.
pub fn file_etag(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        context.update(&buffer[..read]);
    }
    let digest = context.finish();
    let hex: String = digest.as_ref()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(format!("\"wfb1-{hex}\""))
}

/// Prefix of the temporary directory each bundle is written to (next to `waveforms.db`).
pub const BUNDLE_DIR_PREFIX: &str = "audiolib-wfbundle-";

/// Remove `audiolib-wfbundle-*` entries in `dir` last modified at least `min_age` ago
/// (left behind when the server was killed while building or sending a bundle).
/// Best effort; returns how many entries were removed.
pub fn sweep_stale_bundles(dir: &Path, min_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(BUNDLE_DIR_PREFIX)
        {
            continue;
        }
        let old_enough = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_none_or(|age| age >= min_age);
        if !old_enough {
            continue;
        }
        let path = entry.path();
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(()) => removed += 1,
            Err(error) => tracing::warn!(%error, "Cannot remove a stale waveforms bundle"),
        }
    }
    if removed > 0 {
        tracing::info!(removed, "Removed stale waveforms bundle directories");
    }
    removed
}

/// One `GET /waveforms/bundle` preparation at a time; others get 503 `BUSY`.
pub static BUNDLE_SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

impl Drop for WaveformsDb {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Client-side validation of a bundle (doc §9.4.2), also used by the tests: integrity
/// check, table/column whitelist, 256-byte blobs, encoding and `n` range.
pub fn validate_bundle(path: &Path) -> std::result::Result<(), String> {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let check: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if check != "ok" {
        return Err(format!("integrity_check: {check}"));
    }
    let mut tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
        .map_err(|e| e.to_string())?;
    tables.sort();
    if tables != ["list_waveforms", "meta"] {
        return Err(format!("unexpected tables {tables:?}"));
    }
    let columns = |table: &str| -> std::result::Result<Vec<String>, String> {
        conn.prepare(&format!("PRAGMA table_info({table})"))
            .and_then(|mut s| s.query_map([], |r| r.get(1))?.collect())
            .map_err(|e| e.to_string())
    };
    if columns("list_waveforms")?
        != [
            "file_id",
            "etag",
            "src_mtime",
            "src_size",
            "encoding",
            "n",
            "blob",
        ]
    {
        return Err("unexpected list_waveforms columns".into());
    }
    if columns("meta")? != ["key", "value"] {
        return Err("unexpected meta columns".into());
    }
    let bad: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM list_waveforms WHERE typeof(blob) != 'blob'
               OR length(blob) != ?1 OR encoding != ?2 OR n < 0 OR n > 128",
            params![BLOB_BYTES as i64, I8_ENCODING],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if bad != 0 {
        return Err(format!("{bad} invalid rows"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantization_vectors() {
        for (i16v, i8v) in [
            (0, 0),
            (32767, 127),
            (-32767, -127),
            (-32768, -127),
            (16384, 64),
            (-16384, -64),
            (16383, 63),
            (128, 0),
            (129, 0),
            (130, 1),
            (-130, -1),
            (258, 1),
            (387, 1),
            (388, 2),
            (32638, 127),
            (32509, 126),
        ] {
            assert_eq!(i16_to_i8(i16v), i8v, "{i16v}");
            let expected = (f64::from(i16v.max(-32767)) * 127.0 / 32767.0).round() as i8;
            assert_eq!(i16_to_i8(i16v), expected);
        }
    }

    #[test]
    fn never_emits_minus_128_and_is_monotonic() {
        let mut last = i8::MIN;
        for v in i16::MIN..=i16::MAX {
            let q = i16_to_i8(v);
            assert_ne!(q, i8::MIN, "{v}");
            assert!(q >= last);
            last = q;
        }
    }

    #[test]
    fn base64_length_formula() {
        assert_eq!(base64_len(0), 0);
        assert_eq!(base64_len(1), 4);
        assert_eq!(base64_len(125), 336);
        assert_eq!(base64_len(128), 344);
        for n in 0..=128 {
            let bytes = vec![0_u8; 2 * n];
            assert_eq!(crate::list_waveform::base64(&bytes).len(), base64_len(n));
        }
    }

    #[test]
    fn blob_is_padded_to_256_bytes() {
        assert_eq!(to_blob(&[]).len(), BLOB_BYTES);
        let blob = to_blob(&[-32767, 32767]);
        assert_eq!(&blob[..3], &[0x81, 0x7f, 0]);
        assert_eq!(blob.len(), BLOB_BYTES);
    }

    #[test]
    fn upsert_and_bundle_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let db = WaveformsDb::open(dir.path()).unwrap();
        db.connection(true).unwrap();
        let mode: String = lock(&db.conn)
            .as_ref()
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        for id in 1..=300 {
            db.queue(Row {
                file_id: id,
                etag: format!("e{id}"),
                src_mtime: 1,
                src_size: 2,
                n: 128,
                blob: to_blob(&[1000; 256]),
            });
        }
        assert_eq!(db.etag(300).as_deref(), Some("e300"));
        db.queue(Row {
            file_id: 7,
            etag: "new".into(),
            src_mtime: 3,
            src_size: 4,
            n: 1,
            blob: to_blob(&[0, 0]),
        });
        db.flush();
        assert_eq!(db.count(), 300);
        assert_eq!(db.get(7).unwrap().etag, "new");
        let out = dir.path().join("bundle.db");
        let etag = db.bundle(&out, "lib", 42).unwrap();
        assert_eq!(etag, file_etag(&out).unwrap());
        validate_bundle(&out).unwrap();
        let conn = Connection::open(&out).unwrap();
        let rev: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'revision'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(rev, "42");
        db.retain(|id| id <= 10);
        assert_eq!(db.count(), 10);
    }

    #[test]
    fn writes_during_bundling_are_found_again_by_etag() {
        let dir = tempfile::tempdir().unwrap();
        let db = std::sync::Arc::new(WaveformsDb::open(dir.path()).unwrap());
        let mk = |id: i64, v: u32| Row {
            file_id: id,
            etag: format!("e{id}-{v}"),
            src_mtime: 1,
            src_size: 1,
            n: 128,
            blob: to_blob(&[100; 256]),
        };
        for id in 1..=20_000 {
            db.queue(mk(id, 0));
        }
        db.flush();
        let writer = {
            let db = db.clone();
            std::thread::spawn(move || {
                for round in 1..=20 {
                    for id in (round..=20_000).step_by(97) {
                        db.queue(mk(id, round as u32));
                    }
                    db.flush();
                }
            })
        };
        let out = dir.path().join("b.db");
        let etag = db.bundle(&out, "lib", 1).unwrap();
        writer.join().unwrap();
        db.flush();
        assert!(etag.starts_with("\"wfb1-"));
        validate_bundle(&out).unwrap();
        // Client resume: every row whose etag differs from the bundle is re-fetched,
        // so the final state equals the server's.
        let bundle = Connection::open(&out).unwrap();
        let mut stale = 0;
        for id in 1..=20_000 {
            let server = db.etag(id).unwrap();
            let local: String = bundle
                .query_row(
                    "SELECT etag FROM list_waveforms WHERE file_id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            if local != server {
                stale += 1;
            }
        }
        // No write made during bundling was lost, and a client resuming from the
        // bundle by etag re-fetches exactly the rows written after the snapshot.
        let mut expected_stale = 0;
        for id in 1..=20_000_i64 {
            let last = (1..=20_i64)
                .rev()
                .find(|r| id >= *r && (id - r) % 97 == 0)
                .unwrap_or(0);
            assert_eq!(db.etag(id).unwrap(), format!("e{id}-{last}"));
            let local: String = bundle
                .query_row(
                    "SELECT etag FROM list_waveforms WHERE file_id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            let bundled_version: i64 = local.rsplit('-').next().unwrap().parse().unwrap();
            assert!(bundled_version <= last);
            if bundled_version < last {
                expected_stale += 1;
            }
        }
        assert_eq!(stale, expected_stale);
        assert_eq!(db.count(), 20_000);
    }

    #[test]
    fn validation_rejects_bad_blobs_and_extra_tables() {
        let dir = tempfile::tempdir().unwrap();
        let db = WaveformsDb::open(dir.path()).unwrap();
        db.flush();
        let out = dir.path().join("b.db");
        db.bundle(&out, "lib", 1).unwrap();
        let conn = Connection::open(&out).unwrap();
        conn.execute(
            "INSERT INTO list_waveforms VALUES (1, 'e', 0, 0, ?1, 1, x'00')",
            [I8_ENCODING],
        )
        .unwrap();
        assert!(validate_bundle(&out).is_err());
        conn.execute("DELETE FROM list_waveforms", []).unwrap();
        validate_bundle(&out).unwrap();
        conn.execute("CREATE TABLE evil(x)", []).unwrap();
        assert!(validate_bundle(&out).is_err());
    }

    #[test]
    fn stale_bundle_dirs_are_swept_on_open_and_by_age() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(format!("{BUNDLE_DIR_PREFIX}abc"));
        std::fs::create_dir(&stale).unwrap();
        std::fs::write(stale.join("waveforms-bundle.db"), b"x").unwrap();
        let stray_file = dir.path().join(format!("{BUNDLE_DIR_PREFIX}file"));
        std::fs::write(&stray_file, b"x").unwrap();
        let other = dir.path().join("keep-me");
        std::fs::create_dir(&other).unwrap();

        // Too young for a one-hour threshold: kept.
        assert_eq!(
            sweep_stale_bundles(dir.path(), Duration::from_secs(3600)),
            0
        );
        assert!(stale.exists());

        // Opening the store removes every leftover bundle, nothing else.
        let db = WaveformsDb::open(dir.path()).unwrap();
        assert!(!stale.exists() && !stray_file.exists());
        assert!(other.exists());
        assert_eq!(db.path(), dir.path().join(FILE_NAME));
        assert_eq!(
            sweep_stale_bundles(&dir.path().join("missing"), Duration::ZERO),
            0
        );
    }
}
