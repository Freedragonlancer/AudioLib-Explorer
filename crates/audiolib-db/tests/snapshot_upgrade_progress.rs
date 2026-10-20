// crates/audiolib-db/tests/snapshot_upgrade_progress.rs
//! Owned upgrades expose real batch counts and retain one atomic search revision.
use audiolib_core::{DataProvider, Error, SearchRequest};
use audiolib_db::{SnapshotUpgradeProgress, SqliteProvider};
use rusqlite::{Connection, params};
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

fn legacy_fixture(path: &Path, records: usize) {
    drop(SqliteProvider::open(path).unwrap());
    let mut conn = Connection::open(path).unwrap();
    let table: String = conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='audio_search'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let triggers: Vec<String> = conn
        .prepare(
            "SELECT sql FROM sqlite_schema WHERE type='trigger'
             AND name IN ('audio_files_ai','audio_files_au','audio_files_ad') ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute_batch(
        "DROP TRIGGER audio_files_ai; DROP TRIGGER audio_files_au;
         DROP TRIGGER audio_files_ad; DROP TABLE audio_search;",
    )
    .unwrap();
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO audio_files(id,file_path,filename,filename_tokenized,
                 user_tags,user_tags_tokenized,user_notes,user_notes_tokenized,metadata_json)
                 VALUES(?1,?2,?3,?3,'retain tags','retain tags','retain notes','retain notes',?4)",
            )
            .unwrap();
        for index in 0..records {
            let id = if index == 0 {
                i64::MIN
            } else if index + 1 == records {
                i64::MAX
            } else {
                index as i64 - (records / 2) as i64
            };
            let filename = format!("explosion_{index}.wav");
            insert
                .execute(params![
                    id,
                    format!("N:/synthetic/{filename}"),
                    filename,
                    r#"{"private":"keep","wav_metadata":{"fields":{"keywords":"Outdoor"}}}"#,
                ])
                .unwrap();
        }
    }
    tx.execute_batch(&table.replace(
        "tokenize='unicode61 remove_diacritics 2'",
        "tokenize='porter unicode61 remove_diacritics 2'",
    ))
    .unwrap();
    for trigger in triggers {
        tx.execute_batch(&trigger).unwrap();
    }
    tx.execute_batch(
        "INSERT INTO audio_search(audio_search) VALUES('rebuild');
         UPDATE audiolib_schema_features SET version=1 WHERE feature='tokenization';",
    )
    .unwrap();
    tx.commit().unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
}

fn assert_no_sidecars(path: &Path) {
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        assert!(!Path::new(&sidecar).exists(), "{sidecar:?}");
    }
}

#[test]
fn progress_counts_every_signed_id_and_reuses_the_completed_upgrade() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("old.db");
    legacy_fixture(&database, 10_003);
    assert!(SqliteProvider::check_owned_file(&database).unwrap().is_ok());
    let cancel = AtomicBool::new(false);
    let mut events = Vec::new();
    assert!(
        SqliteProvider::upgrade_owned_snapshot_with_progress_cancellable(
            &database,
            &cancel,
            |event| events.push(event),
        )
        .unwrap()
    );
    assert_eq!(events.first(), Some(&SnapshotUpgradeProgress::Preparing));
    let counts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SnapshotUpgradeProgress::Tokenizing { completed, total } => Some((*completed, *total)),
            _ => None,
        })
        .collect();
    assert_eq!(
        counts,
        [
            (0, 10_003),
            (5_000, 10_003),
            (10_000, 10_003),
            (10_003, 10_003)
        ]
    );
    assert_eq!(
        &events[events.len() - 2..],
        [
            SnapshotUpgradeProgress::BuildingFts,
            SnapshotUpgradeProgress::SourceTags
        ]
    );
    assert!(SqliteProvider::has_current_search_state(&database).unwrap());
    assert!(SqliteProvider::check_owned_file(&database).unwrap().is_ok());
    assert_no_sidecars(&database);
    let provider = SqliteProvider::open_read_only(&database).unwrap();
    assert_eq!(
        provider
            .search(&SearchRequest {
                query: "plos".into(),
                ..Default::default()
            })
            .unwrap()
            .total_count,
        10_003
    );
    for id in [i64::MIN, -1, 0, i64::MAX] {
        let audio = provider.get_audio(id).unwrap();
        assert_eq!(audio.tags, "retain tags");
        assert_eq!(audio.notes, "retain notes");
        assert_eq!(audio.attributes["private"], "keep");
    }
    drop(provider);
    let original = fs::read(&database).unwrap();
    events.clear();
    assert!(
        !SqliteProvider::upgrade_owned_snapshot_with_progress_cancellable(
            &database,
            &cancel,
            |event| events.push(event),
        )
        .unwrap()
    );
    assert_eq!(events, [SnapshotUpgradeProgress::Preparing]);
    assert_eq!(fs::read(&database).unwrap(), original);
}

#[test]
fn cancelling_after_a_batch_or_before_fts_rolls_back_every_batch_and_revision() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("old.db");
    legacy_fixture(&source, 5_003);
    let original = fs::read(&source).unwrap();
    for stage in ["batch", "fts"] {
        let database = temp.path().join(format!("cancel-{stage}.db"));
        fs::copy(&source, &database).unwrap();
        let cancel = AtomicBool::new(false);
        let mut reached = false;
        let result = SqliteProvider::upgrade_owned_snapshot_with_progress_cancellable(
            &database,
            &cancel,
            |event| {
                let stop = match event {
                    SnapshotUpgradeProgress::Tokenizing { completed, .. } => {
                        stage == "batch" && completed == 5_000
                    }
                    SnapshotUpgradeProgress::BuildingFts => stage == "fts",
                    _ => false,
                };
                if stop {
                    reached = true;
                    cancel.store(true, Ordering::Release);
                }
            },
        );
        assert!(reached, "{stage}");
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
        assert_eq!(fs::read(&database).unwrap(), original, "{stage}");
        assert!(!SqliteProvider::has_current_search_text(&database).unwrap());
        assert!(SqliteProvider::check_owned_file(&database).unwrap().is_ok());
        assert_no_sidecars(&database);
    }
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[test]
fn empty_snapshot_reports_zero_rows_and_finishes_the_fts_upgrade() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("empty.db");
    legacy_fixture(&database, 0);
    let mut events = Vec::new();
    assert!(
        SqliteProvider::upgrade_owned_snapshot_with_progress_cancellable(
            &database,
            &AtomicBool::new(false),
            |event| events.push(event),
        )
        .unwrap()
    );
    assert_eq!(
        events,
        [
            SnapshotUpgradeProgress::Preparing,
            SnapshotUpgradeProgress::Tokenizing {
                completed: 0,
                total: 0
            },
            SnapshotUpgradeProgress::BuildingFts,
            SnapshotUpgradeProgress::SourceTags,
        ]
    );
    assert_no_sidecars(&database);
    assert!(SqliteProvider::has_current_search_state(&database).unwrap());
}
