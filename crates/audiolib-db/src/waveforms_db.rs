// crates/audiolib-db/src/waveforms_db.rs
//! `waveforms.db`: a separate SQLite file next to (never inside) a library database or a
//! server library's client cache, holding compact list Waveforms (`i8-minmax-base64`
//! bytes, at most 256 per row). WAL mode, one row per record, written by per-row UPSERT.
//! The library schema, its indexes, FTS5 and server snapshots are not touched.
//!
//! Local mode fills it from existing `waveform_cache` `list128-v4-l1` rows
//! ([`crate::SqliteProvider::derive_i8_list_waveforms`], no audio decode); server mode
//! stores the server's i8 items together with their entity tag and encoding.
use crate::db_error;
use audiolib_core::{Error, LIST_WAVEFORM_I8_MAX_BYTES, ListWaveformI8, Result};
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// File name of the compact list Waveform store.
pub const WAVEFORMS_DB_FILE: &str = "waveforms.db";

/// One stored compact list Waveform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveformsDbRow {
    pub file_id: i64,
    /// Server entity tag (full, including its `-l1i8` suffix) or the local zoom key.
    pub etag: String,
    /// Wire encoding the peaks were received or derived in.
    pub encoding: Option<String>,
    /// Source file mtime (Unix nanoseconds) when known (local mode).
    pub src_mtime: Option<i64>,
    /// Source file size in bytes when known (local mode).
    pub src_size: Option<i64>,
    pub peaks: ListWaveformI8,
}

/// Handle of one `waveforms.db` file (a single serialized connection).
pub struct WaveformsDb {
    path: PathBuf,
    conn: Mutex<Connection>,
}

impl WaveformsDb {
    /// `waveforms.db` in the directory of `main_db` (a sibling file, never the same file).
    pub fn path_beside(main_db: &Path) -> PathBuf {
        main_db
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(WAVEFORMS_DB_FILE)
    }

    /// Open or create the store at `path` (WAL mode, table created if absent).
    pub fn open(path: &Path) -> Result<Self> {
        if path.file_name().and_then(|name| name.to_str()) != Some(WAVEFORMS_DB_FILE) {
            return Err(Error::Validation(format!(
                "list Waveform store must be named {WAVEFORMS_DB_FILE}"
            )));
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| Error::Database(format!("{}: {error}", parent.display())))?;
        }
        let conn = Connection::open(path).map_err(db_error)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db_error)?;
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(db_error)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(Error::Database(format!(
                "waveforms.db journal mode is {mode}, expected wal"
            )));
        }
        conn.execute_batch(
            "PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS list_waveforms_i8(
               file_id INTEGER PRIMARY KEY,
               etag TEXT NOT NULL,
               encoding TEXT,
               src_mtime INTEGER,
               src_size INTEGER,
               peaks BLOB CHECK(length(peaks)<=256)
             );",
        )
        .map_err(db_error)?;
        Ok(Self {
            path: path.to_path_buf(),
            conn: Mutex::new(conn),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Insert or replace one row (single-statement UPSERT, its own transaction).
    pub fn upsert(&self, row: &WaveformsDbRow) -> Result<()> {
        let peaks = row.peaks.to_bytes();
        if peaks.len() > LIST_WAVEFORM_I8_MAX_BYTES {
            return Err(Error::Validation("i8 peaks exceed 256 bytes".into()));
        }
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        conn.prepare_cached(
            "INSERT INTO list_waveforms_i8(file_id,etag,encoding,src_mtime,src_size,peaks)
             VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(file_id) DO UPDATE SET etag=excluded.etag,encoding=excluded.encoding,
              src_mtime=excluded.src_mtime,src_size=excluded.src_size,peaks=excluded.peaks",
        )
        .map_err(db_error)?
        .execute(rusqlite::params![
            row.file_id,
            row.etag,
            row.encoding,
            row.src_mtime,
            row.src_size,
            peaks
        ])
        .map_err(db_error)?;
        Ok(())
    }

    /// The row of `file_id`. A stored blob that no longer validates is reported as absent.
    pub fn get(&self, file_id: i64) -> Result<Option<WaveformsDbRow>> {
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        let row = conn
            .prepare_cached(
                "SELECT etag,encoding,src_mtime,src_size,peaks FROM list_waveforms_i8
                 WHERE file_id=?1",
            )
            .map_err(db_error)?
            .query_row([file_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                ))
            })
            .optional()
            .map_err(db_error)?;
        Ok(
            row.and_then(|(etag, encoding, src_mtime, src_size, peaks)| {
                let peaks = ListWaveformI8::from_bytes(&peaks.unwrap_or_default()).ok()?;
                Some(WaveformsDbRow {
                    file_id,
                    etag,
                    encoding,
                    src_mtime,
                    src_size,
                    peaks,
                })
            }),
        )
    }

    /// Rows of several ids (absent ones left out).
    pub fn get_many(&self, ids: &[i64]) -> Result<Vec<WaveformsDbRow>> {
        let mut rows = Vec::new();
        for id in ids {
            if let Some(row) = self.get(*id)? {
                rows.push(row);
            }
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audiolib_core::LIST_WAVEFORM_ENCODING_I8;

    fn row(id: i64, etag: &str, values: Vec<i8>) -> WaveformsDbRow {
        WaveformsDbRow {
            file_id: id,
            etag: etag.into(),
            encoding: Some(LIST_WAVEFORM_ENCODING_I8.into()),
            src_mtime: Some(11),
            src_size: Some(22),
            peaks: ListWaveformI8::new(values).unwrap(),
        }
    }

    #[test]
    fn upsert_replaces_rows_and_survives_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("library.db");
        let path = WaveformsDb::path_beside(&main);
        assert_eq!(path, directory.path().join("waveforms.db"));
        {
            let db = WaveformsDb::open(&path).unwrap();
            db.upsert(&row(7, "\"a-l1i8\"", vec![-1, 1])).unwrap();
            db.upsert(&row(8, "\"b-l1i8\"", vec![])).unwrap();
            db.upsert(&row(7, "\"c-l1i8\"", vec![-5, 5, 0, 1])).unwrap();
        }
        let db = WaveformsDb::open(&path).unwrap();
        let mode: String = db
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        let seven = db.get(7).unwrap().unwrap();
        assert_eq!(seven.etag, "\"c-l1i8\"");
        assert_eq!(seven.peaks.values, vec![-5, 5, 0, 1]);
        assert_eq!(seven.encoding.as_deref(), Some(LIST_WAVEFORM_ENCODING_I8));
        assert_eq!(db.get(8).unwrap().unwrap().peaks.columns(), 0);
        assert!(db.get(9).unwrap().is_none());
        assert_eq!(db.get_many(&[9, 8, 7]).unwrap().len(), 2);
        let count: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM list_waveforms_i8", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn oversized_blob_is_rejected_by_the_table_check() {
        let directory = tempfile::tempdir().unwrap();
        let db = WaveformsDb::open(&directory.path().join(WAVEFORMS_DB_FILE)).unwrap();
        let error = db.conn.lock().unwrap().execute(
            "INSERT INTO list_waveforms_i8(file_id,etag,peaks) VALUES(1,'x',zeroblob(257))",
            [],
        );
        assert!(error.is_err());
        assert!(WaveformsDb::open(&directory.path().join("library.db")).is_err());
    }
}
