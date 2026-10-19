// crates/audiolib-db/src/list_waveforms.rs
//! Local list thumbnail Waveforms (doc/api/README.md §9.3) in the existing
//! `waveform_cache` table: one row per record with `zoom_level = 'list128-v4-l1'`,
//! `min_values` / `max_values` = `n` int16 little endian each and NULL `rms_values` /
//! `color_values`. No schema change. Lookups never decode audio; a row is current while
//! the file on disk still has the size and mtime of the library record (or of the file
//! version it was computed from in this session).
use crate::{SqliteProvider, db_error};
use audiolib_core::{
    Error, ListWaveform, ListWaveformBatch, ListWaveformItem, ListWaveformState,
    MAX_LIST_WAVEFORM_IDS, Result,
};
use rusqlite::OptionalExtension;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::UNIX_EPOCH,
};

/// `waveform_cache.zoom_level` of list Waveforms (same value as
/// `audiolib_audio::LIST_WAVEFORM_ZOOM_LEVEL`, which needs the scanner feature).
pub(crate) const LIST_ZOOM_LEVEL: &str = "list128-v4-l1";

/// File size and modification time in Unix nanoseconds (as `audio_files` stores them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileVersion {
    size: u64,
    mtime_ns: Option<i64>,
}

impl FileVersion {
    fn of(path: &Path) -> std::io::Result<Self> {
        let metadata = fs::metadata(path)?;
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|elapsed| i64::try_from(elapsed.as_nanos()).ok());
        Ok(Self {
            size: metadata.len(),
            mtime_ns,
        })
    }

    /// Whether the file on disk (`disk`) is still the version this record describes.
    /// Legacy records without an mtime compare by size only.
    fn describes(self, disk: Self) -> bool {
        self.size == disk.size && (self.mtime_ns.is_none() || self.mtime_ns == disk.mtime_ns)
    }
}

/// Session memory of the provider: file versions that differ from their (not yet
/// rescanned) record but whose rows were computed from that version, and decode
/// failures, so neither is recomputed while the file stays unchanged.
#[derive(Default)]
pub(crate) struct ListWaveformMemory {
    computed: Mutex<HashMap<i64, FileVersion>>,
    // Written by the decoder (`scanner` feature) only.
    #[cfg_attr(not(feature = "scanner"), allow(dead_code))]
    failed: Mutex<HashMap<i64, (FileVersion, String)>>,
}

struct Record {
    path: String,
    version: FileVersion,
    missing: bool,
}

type Stored = Option<(u32, Vec<u8>, Vec<u8>)>;

impl SqliteProvider {
    fn list_record(conn: &rusqlite::Connection, id: i64) -> Result<Option<(Record, Stored)>> {
        conn.prepare_cached(
            "SELECT a.file_path,COALESCE(a.file_size,0),a.source_mtime_ns,COALESCE(a.missing,0),
             w.samples_per_pixel,w.min_values,w.max_values
             FROM audio_files a LEFT JOIN waveform_cache w ON w.id=(
              SELECT MAX(c.id) FROM waveform_cache c WHERE c.audio_file_id=a.id AND c.zoom_level=?2)
             WHERE a.id=?1",
        )
        .map_err(db_error)?
        .query_row(rusqlite::params![id, LIST_ZOOM_LEVEL], |row| {
            let stored = match (
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<Vec<u8>>>(5)?,
                row.get::<_, Option<Vec<u8>>>(6)?,
            ) {
                (samples, Some(min), Some(max)) => Some((
                    samples
                        .and_then(|value| u32::try_from(value).ok())
                        .unwrap_or(1),
                    min,
                    max,
                )),
                _ => None,
            };
            Ok((
                Record {
                    path: row.get(0)?,
                    version: FileVersion {
                        size: crate::unsigned(row, 1)?,
                        mtime_ns: row.get(2)?,
                    },
                    missing: row.get::<_, i64>(3)? != 0,
                },
                stored,
            ))
        })
        .optional()
        .map_err(db_error)
    }

    fn list_state(&self, id: i64, record: Option<(Record, Stored)>) -> ListWaveformState {
        let Some((record, stored)) = record else {
            return ListWaveformState::Missing;
        };
        if record.missing {
            return ListWaveformState::Missing;
        }
        let Some((samples_per_pixel, min, max)) = stored else {
            return ListWaveformState::Pending { previous: None };
        };
        let disk = match FileVersion::of(Path::new(&record.path)) {
            Ok(disk) => disk,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ListWaveformState::Missing;
            }
            Err(error) => {
                return ListWaveformState::Failed {
                    message: error.to_string(),
                };
            }
        };
        let current = record.version.describes(disk)
            || self.list_waveform_memory.computed.lock().unwrap().get(&id) == Some(&disk);
        match ListWaveform::from_min_max_le(samples_per_pixel, &min, &max) {
            // A damaged row is recomputed like a stale one.
            Ok(waveform) if current => ListWaveformState::Ready { waveform },
            _ => ListWaveformState::Pending { previous: None },
        }
    }

    pub(crate) fn list_waveforms_impl(
        &self,
        ids: &[i64],
        cancel: &AtomicBool,
    ) -> Result<ListWaveformBatch> {
        if self.read_only {
            // A server snapshot's rows may come from an old local library (§9.3).
            return Err(Error::Validation(
                "A read-only library has no list Waveforms".into(),
            ));
        }
        if ids.len() > MAX_LIST_WAVEFORM_IDS {
            return Err(Error::Validation(format!(
                "At most {MAX_LIST_WAVEFORM_IDS} list Waveforms per request"
            )));
        }
        let mut seen = HashSet::new();
        let mut items = Vec::with_capacity(ids.len());
        for &id in ids {
            if !seen.insert(id) {
                continue;
            }
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            // Read the row, release the connection, then read the file Metadata.
            let record = Self::list_record(&*self.reader()?, id)?;
            items.push(ListWaveformItem {
                id,
                state: self.list_state(id, record),
            });
        }
        Ok(ListWaveformBatch {
            items,
            retry_after: None,
            deferred: None,
        })
    }

    pub(crate) fn list_waveform_backlog_impl(&self, after: i64, limit: usize) -> Result<Vec<i64>> {
        if self.read_only {
            return Ok(Vec::new());
        }
        let conn = self.reader()?;
        let mut statement = conn
            .prepare_cached(
                "SELECT a.id FROM audio_files a WHERE a.missing=0 AND a.id>?1
                 AND NOT EXISTS (SELECT 1 FROM waveform_cache w
                  WHERE w.audio_file_id=a.id AND w.zoom_level=?2)
                 ORDER BY a.id LIMIT ?3",
            )
            .map_err(db_error)?;
        statement
            .query_map(
                rusqlite::params![
                    after,
                    LIST_ZOOM_LEVEL,
                    i64::try_from(limit).unwrap_or(i64::MAX)
                ],
                |row| row.get(0),
            )
            .map_err(db_error)?
            .collect::<rusqlite::Result<_>>()
            .map_err(db_error)
    }

    /// Replace the record's list row, unless the record changed (rescanned or removed)
    /// since `expected` was read. Returns whether the row was written.
    #[cfg_attr(not(feature = "scanner"), allow(dead_code))]
    fn store_list_waveform(
        &self,
        id: i64,
        expected: FileVersion,
        waveform: &ListWaveform,
    ) -> Result<bool> {
        let mut writer = self.writer()?;
        let tx = writer.transaction().map_err(db_error)?;
        let current = tx
            .query_row(
                "SELECT COALESCE(file_size,0),source_mtime_ns FROM audio_files WHERE id=?1 AND missing=0",
                [id],
                |row| {
                    Ok(FileVersion {
                        size: crate::unsigned(row, 0)?,
                        mtime_ns: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(db_error)?;
        if current != Some(expected) {
            return Ok(false);
        }
        tx.execute(
            "DELETE FROM waveform_cache WHERE audio_file_id=?1 AND zoom_level=?2",
            rusqlite::params![id, LIST_ZOOM_LEVEL],
        )
        .map_err(db_error)?;
        tx.execute(
            "INSERT INTO waveform_cache(audio_file_id,zoom_level,samples_per_pixel,min_values,
             max_values,rms_values,color_values,created_at)
             VALUES(?1,?2,?3,?4,?5,NULL,NULL,strftime('%Y-%m-%d %H:%M:%f','now'))",
            rusqlite::params![
                id,
                LIST_ZOOM_LEVEL,
                i64::from(waveform.samples_per_pixel),
                waveform.min_values_le(),
                waveform.max_values_le()
            ],
        )
        .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(true)
    }

    #[cfg(feature = "scanner")]
    pub(crate) fn compute_list_waveform_impl(
        &self,
        id: i64,
        cancel: &AtomicBool,
    ) -> Result<ListWaveformState> {
        if self.read_only {
            return Err(Error::Validation(
                "A read-only library has no list Waveforms".into(),
            ));
        }
        let Some((record, _)) = Self::list_record(&*self.reader()?, id)? else {
            return Ok(ListWaveformState::Missing);
        };
        if record.missing {
            return Ok(ListWaveformState::Missing);
        }
        let path = Path::new(&record.path);
        let before = match FileVersion::of(path) {
            Ok(version) => version,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ListWaveformState::Missing);
            }
            Err(error) => return Err(error.into()),
        };
        if let Some((version, message)) = self.list_waveform_memory.failed.lock().unwrap().get(&id)
            && *version == before
        {
            return Ok(ListWaveformState::Failed {
                message: message.clone(),
            });
        }
        let waveform = match audiolib_audio::list_waveform_with_cancel(path, cancel) {
            Ok(waveform) => waveform,
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(Error::Io(error)) => return Err(Error::Io(error)),
            Err(error) => {
                let message = error.to_string();
                self.list_waveform_memory
                    .failed
                    .lock()
                    .unwrap()
                    .insert(id, (before, message.clone()));
                return Ok(ListWaveformState::Failed { message });
            }
        };
        // A file written while it was decoded is computed again later.
        if FileVersion::of(path).ok() != Some(before) {
            return Ok(ListWaveformState::Pending { previous: None });
        }
        self.list_waveform_memory.failed.lock().unwrap().remove(&id);
        if self.store_list_waveform(id, record.version, &waveform)? {
            let mut computed = self.list_waveform_memory.computed.lock().unwrap();
            if record.version.describes(before) {
                computed.remove(&id);
            } else {
                computed.insert(id, before);
            }
        }
        Ok(ListWaveformState::Ready { waveform })
    }
}

#[cfg(all(test, feature = "scanner"))]
mod tests {
    #[test]
    fn zoom_level_matches_the_shared_audio_constant() {
        assert_eq!(
            super::LIST_ZOOM_LEVEL,
            audiolib_audio::LIST_WAVEFORM_ZOOM_LEVEL
        );
    }
}
