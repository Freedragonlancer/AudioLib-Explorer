// crates/audiolib-db/tests/list_waveforms.rs
//! Local list Waveforms (doc/api/README.md §9.3): `waveform_cache` rows with zoom level
//! `list128-v4-l1`, lookups that never decode, on-demand computation, file-change
//! invalidation, the pre-generation backlog and read-only snapshots.
use audiolib_core::{DataProvider, ListWaveformSource, ListWaveformState};
use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider};
use rusqlite::Connection;
use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

fn write_wav(path: &Path, frames: u32, level: i16) {
    let bytes = frames * 2;
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(36 + bytes).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&48000u32.to_le_bytes());
    data.extend_from_slice(&96000u32.to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&bytes.to_le_bytes());
    for frame in 0..frames {
        let value = if frame % 2 == 0 { level } else { -level };
        data.extend_from_slice(&value.to_le_bytes());
    }
    fs::write(path, data).unwrap();
}

fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn scan(provider: &Arc<SqliteProvider>, root: &Path) {
    ScanService::new(provider.clone())
        .scan(
            root,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
}

fn id_of(provider: &SqliteProvider, path: &Path) -> i64 {
    provider
        .find_audio_by_path(&path.canonicalize().unwrap())
        .unwrap()
        .unwrap()
        .id
}

/// A stored list row: (zoom level, samples per pixel, min, max, rms is NULL, color is NULL).
type ListRow = (String, i64, Vec<u8>, Vec<u8>, bool, bool);

fn list_rows(database: &Path, id: i64) -> Vec<ListRow> {
    let conn = Connection::open(database).unwrap();
    let mut statement = conn
        .prepare(
            "SELECT zoom_level,samples_per_pixel,min_values,max_values,
             rms_values IS NULL,color_values IS NULL
             FROM waveform_cache WHERE audio_file_id=?1 ORDER BY id",
        )
        .unwrap();
    statement
        .query_map([id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn state(provider: &SqliteProvider, id: i64) -> ListWaveformState {
    let batch = provider.list_waveforms(&[id], cancel()).unwrap();
    assert_eq!(batch.items.len(), 1);
    assert_eq!(batch.items[0].id, id);
    batch.items.into_iter().next().unwrap().state
}

#[test]
fn computed_rows_are_stored_reused_and_invalidated_when_files_change() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("audio");
    fs::create_dir_all(&root).unwrap();
    let long = root.join("long.wav");
    let short = root.join("short.wav");
    let empty = root.join("empty.wav");
    write_wav(&long, 48_000, 16384);
    write_wav(&short, 1000, 32767);
    write_wav(&empty, 0, 0);
    let provider = Arc::new(SqliteProvider::open(&temp.path().join("lib.db")).unwrap());
    let database = provider.database_path().to_path_buf();
    scan(&provider, &root);
    assert_eq!(provider.list_waveform_source(), ListWaveformSource::Local);
    let (long_id, short_id, empty_id) = (
        id_of(&provider, &long),
        id_of(&provider, &short),
        id_of(&provider, &empty),
    );
    // Scanning computes nothing; every lookup is pending and the backlog has all three.
    let mut backlog = provider.list_waveform_backlog(0, 10).unwrap();
    backlog.sort();
    let mut all = vec![long_id, short_id, empty_id];
    all.sort();
    assert_eq!(backlog, all);
    let batch = provider
        .list_waveforms(&[long_id, short_id, long_id, i64::MAX], cancel())
        .unwrap();
    let states: Vec<_> = batch
        .items
        .iter()
        .map(|item| (item.id, &item.state))
        .collect();
    assert_eq!(
        states,
        vec![
            (long_id, &ListWaveformState::Pending { previous: None }),
            (short_id, &ListWaveformState::Pending { previous: None }),
            (i64::MAX, &ListWaveformState::Missing),
        ]
    );

    let ListWaveformState::Ready { waveform } =
        provider.compute_list_waveform(long_id, cancel()).unwrap()
    else {
        panic!("long file must compute");
    };
    assert_eq!((waveform.samples_per_pixel, waveform.columns()), (375, 128));
    assert_eq!(waveform.column(0), Some((-16384, 16384)));
    let rows = list_rows(&database, long_id);
    assert_eq!(rows.len(), 1);
    let (zoom, samples, min, max, rms_null, color_null) = &rows[0];
    assert_eq!((zoom.as_str(), *samples), ("list128-v4-l1", 375));
    assert_eq!((min.len(), max.len()), (256, 256));
    assert_eq!(&min[..2], &(-16384_i16).to_le_bytes());
    assert_eq!(&max[..2], &16384_i16.to_le_bytes());
    assert!(*rms_null && *color_null);
    assert_eq!(
        state(&provider, long_id),
        ListWaveformState::Ready {
            waveform: waveform.clone()
        }
    );
    // Recomputing replaces the single row.
    provider.compute_list_waveform(long_id, cancel()).unwrap();
    assert_eq!(list_rows(&database, long_id).len(), 1);

    // 1000 frames -> 125 columns; a 0-frame file is ready with no columns.
    let ListWaveformState::Ready {
        waveform: short_waveform,
    } = provider.compute_list_waveform(short_id, cancel()).unwrap()
    else {
        panic!("short file must compute");
    };
    assert_eq!(short_waveform.columns(), 125);
    let ListWaveformState::Ready {
        waveform: empty_waveform,
    } = provider.compute_list_waveform(empty_id, cancel()).unwrap()
    else {
        panic!("empty file must compute");
    };
    assert_eq!(empty_waveform.columns(), 0);
    assert_eq!(list_rows(&database, empty_id)[0].2.len(), 0);
    assert!(provider.list_waveform_backlog(0, 10).unwrap().is_empty());

    // Changed on disk without a rescan: stale until recomputed, then current for the
    // new file version even though the record still describes the old one.
    write_wav(&long, 24_000, 8192);
    assert_eq!(
        state(&provider, long_id),
        ListWaveformState::Pending { previous: None }
    );
    let ListWaveformState::Ready { waveform: changed } =
        provider.compute_list_waveform(long_id, cancel()).unwrap()
    else {
        panic!("changed file must compute");
    };
    assert_eq!(changed.column(0), Some((-8192, 8192)));
    assert_eq!(
        state(&provider, long_id),
        ListWaveformState::Ready { waveform: changed }
    );

    // A rescan of the changed file drops its row; a missing file is reported missing.
    write_wav(&long, 12_000, 4096);
    scan(&provider, &root);
    assert!(list_rows(&database, long_id).is_empty());
    assert_eq!(
        provider.list_waveform_backlog(0, 10).unwrap(),
        vec![long_id]
    );
    fs::remove_file(&short).unwrap();
    assert_eq!(state(&provider, short_id), ListWaveformState::Missing);
    assert_eq!(
        provider.compute_list_waveform(short_id, cancel()).unwrap(),
        ListWaveformState::Missing
    );
}

#[test]
fn undecodable_files_fail_once_and_cancellation_writes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("audio");
    fs::create_dir_all(&root).unwrap();
    let good = root.join("good.wav");
    write_wav(&good, 48_000, 1000);
    let provider = Arc::new(SqliteProvider::open(&temp.path().join("lib.db")).unwrap());
    scan(&provider, &root);
    let id = id_of(&provider, &good);
    let cancelled = cancel();
    cancelled.store(true, std::sync::atomic::Ordering::Release);
    assert!(matches!(
        provider.compute_list_waveform(id, cancelled.clone()),
        Err(audiolib_core::Error::Cancelled)
    ));
    assert!(matches!(
        provider.list_waveforms(&[id], cancelled),
        Err(audiolib_core::Error::Cancelled)
    ));
    assert!(list_rows(provider.database_path(), id).is_empty());
    // Truncate the data so the decode fails after the scan accepted the header.
    let bytes = fs::read(&good).unwrap();
    fs::write(&good, &bytes[..bytes.len() / 2]).unwrap();
    let first = provider.compute_list_waveform(id, cancel()).unwrap();
    assert!(
        matches!(first, ListWaveformState::Failed { .. }),
        "{first:?}"
    );
    assert_eq!(provider.compute_list_waveform(id, cancel()).unwrap(), first);
    assert!(list_rows(provider.database_path(), id).is_empty());
}

#[test]
fn read_only_snapshots_do_not_offer_their_waveform_cache_rows() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("audio");
    fs::create_dir_all(&root).unwrap();
    let file = root.join("a.wav");
    write_wav(&file, 4800, 1000);
    let provider = Arc::new(SqliteProvider::open(&temp.path().join("lib.db")).unwrap());
    scan(&provider, &root);
    let id = id_of(&provider, &file);
    provider.compute_list_waveform(id, cancel()).unwrap();
    let database = provider.database_path().to_path_buf();
    drop(provider);
    let snapshot = SqliteProvider::open_read_only(&database).unwrap();
    assert_eq!(
        snapshot.list_waveform_source(),
        ListWaveformSource::Unavailable
    );
    assert!(snapshot.list_waveforms(&[id], cancel()).is_err());
    assert!(snapshot.compute_list_waveform(id, cancel()).is_err());
    assert!(snapshot.list_waveform_backlog(0, 10).unwrap().is_empty());
}
