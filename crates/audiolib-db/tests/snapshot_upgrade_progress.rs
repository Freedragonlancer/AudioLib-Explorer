// crates/audiolib-db/tests/snapshot_upgrade_progress.rs
//! Owned atomic upgrades and resumable live upgrades preserve records across cancellation.
use audiolib_core::{DataProvider, Error, SearchRequest};
use audiolib_db::{SnapshotUpgradeProgress, SqliteProvider};
use rusqlite::{Connection, params};
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[test]
fn cancelled_snapshot_does_not_publish_or_overwrite_its_destination() {
    let dir = tempfile::tempdir().unwrap();
    let provider = SqliteProvider::open(&dir.path().join("library.db")).unwrap();
    let destination = dir.path().join("backup.db");
    fs::write(&destination, b"retain existing backup").unwrap();
    let result = provider.backup_library_cancellable(&destination, &AtomicBool::new(true));
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(fs::read(&destination).unwrap(), b"retain existing backup");
    assert!(!fs::read_dir(dir.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("audiolib-backup-")
    }));
}

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
    if records > 0 {
        tx.execute(
            "INSERT INTO play_history(audio_file_id,played_at,filename,file_path)
             VALUES(?1,'2026-10-01T00:00:00Z','retained history','retained path')",
            [i64::MIN],
        )
        .unwrap();
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

#[test]
fn live_upgrade_resumes_each_committed_phase_without_repeating_rows() {
    for stop in ["tokenizing", "indexing", "validating", "checkpoint"] {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("live.db");
        legacy_fixture(&database, 5_003);
        let cancel = AtomicBool::new(false);
        let result =
            SqliteProvider::open_existing_with_progress_cancellable(&database, &cancel, |event| {
                let reached = match event {
                    SnapshotUpgradeProgress::Tokenizing {
                        completed: 5000, ..
                    } => stop == "tokenizing",
                    SnapshotUpgradeProgress::Indexing {
                        completed: 5000, ..
                    } => stop == "indexing",
                    SnapshotUpgradeProgress::Validating => stop == "validating",
                    SnapshotUpgradeProgress::Checkpoint => stop == "checkpoint",
                    _ => false,
                };
                if reached {
                    cancel.store(true, Ordering::Release);
                }
            });
        assert!(matches!(result, Err(Error::Cancelled)), "{stop}");
        assert!(matches!(
            SqliteProvider::open_read_only(&database),
            Err(Error::Busy(_))
        ));
        let conn = Connection::open(&database).unwrap();
        let (phase, completed, backup): (String, u64, String) = conn
            .query_row(
                "SELECT phase,completed,rollback FROM audiolib_search_upgrade",
                [],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(phase, stop);
        assert!(Path::new(&backup).is_file());
        assert!(!SqliteProvider::has_current_search_text(Path::new(&backup)).unwrap());
        assert_eq!(
            conn.query_row(
                "SELECT version FROM audiolib_schema_features WHERE feature='tokenization'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            4
        );
        drop(conn);
        let mut events = Vec::new();
        let provider =
            SqliteProvider::open_existing_with_progress(&database, |event| events.push(event))
                .unwrap();
        assert!(!events.contains(&SnapshotUpgradeProgress::BackingUp));
        if stop == "tokenizing" {
            assert_eq!(
                events.iter().find_map(|e| match e {
                    SnapshotUpgradeProgress::Tokenizing { completed, .. } => Some(*completed),
                    _ => None,
                }),
                Some(completed)
            );
        }
        if stop == "indexing" {
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, SnapshotUpgradeProgress::Tokenizing { .. }))
            );
            assert_eq!(
                events.iter().find_map(|e| match e {
                    SnapshotUpgradeProgress::Indexing { completed, .. } => Some(*completed),
                    _ => None,
                }),
                Some(completed)
            );
        }
        assert_eq!(
            provider
                .search(&SearchRequest {
                    query: "plos".into(),
                    ..Default::default()
                })
                .unwrap()
                .total_count,
            5003
        );
        assert_eq!(provider.get_audio(i64::MIN).unwrap().notes, "retain notes");
        assert_eq!(
            provider.get_audio(i64::MAX).unwrap().attributes["private"],
            "keep"
        );
        let audit =
            Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_eq!(audit.query_row(
            "SELECT COUNT(*) FROM audio_files WHERE user_notes!='retain notes' OR user_tags!='retain tags' OR json_extract(metadata_json,'$.private')!='keep'",
            [], |r| r.get::<_, i64>(0),
        ).unwrap(), 0);
        assert_eq!(audit.query_row(
            "SELECT COUNT(*) FROM play_history WHERE filename='retained history' AND file_path='retained path'",
            [], |r| r.get::<_, i64>(0),
        ).unwrap(), 1);
        drop(audit);
        assert_eq!(
            fs::metadata(format!("{}-wal", database.display()))
                .map(|m| m.len())
                .unwrap_or(0),
            0
        );
        assert!(SqliteProvider::check_owned_file(&database).unwrap().is_ok());
        assert_eq!(
            fs::read_dir(temp.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e
                    .file_name()
                    .to_string_lossy()
                    .starts_with("audiolib-rollback-"))
                .count(),
            1
        );
    }
}

#[test]
fn incomplete_upgrade_blocks_preexisting_provider_and_rejects_unknown_protocol() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("live.db");
    legacy_fixture(&database, 3);
    // Complete once, retain a live handle, then simulate a library restored to older derived state.
    let older = SqliteProvider::open_existing(&database).unwrap();
    let conn = Connection::open(&database).unwrap();
    conn.execute(
        "UPDATE audiolib_schema_features SET version=1 WHERE feature='tokenization'",
        [],
    )
    .unwrap();
    drop(conn);
    let cancel = AtomicBool::new(false);
    let result =
        SqliteProvider::open_existing_with_progress_cancellable(&database, &cancel, |event| {
            if matches!(
                event,
                SnapshotUpgradeProgress::Tokenizing { completed: 0, .. }
            ) {
                assert!(matches!(
                    older.search(&SearchRequest::default()),
                    Err(Error::Busy(_))
                ));
                cancel.store(true, Ordering::Release);
            }
        });
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(matches!(older.get_audio(i64::MIN), Err(Error::Busy(_))));
    let conn = Connection::open(&database).unwrap();
    assert!(
        conn.execute("UPDATE audio_files SET user_notes='must be blocked'", [])
            .is_err()
    );
    conn.execute("UPDATE audiolib_search_upgrade SET protocol=99", [])
        .unwrap();
    drop(conn);
    assert!(matches!(
        SqliteProvider::open_existing(&database),
        Err(Error::Database(_))
    ));
}

#[test]
fn checkpoint_reports_pinned_reader_and_truncates_after_it_releases() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("live.db");
    legacy_fixture(&database, 3);
    let provider = SqliteProvider::open_existing(&database).unwrap();
    let reader = Connection::open(&database).unwrap();
    reader
        .execute_batch("BEGIN; SELECT COUNT(*) FROM audio_files;")
        .unwrap();
    let writer = Connection::open(&database).unwrap();
    writer
        .execute("UPDATE audio_files SET favorite=1", [])
        .unwrap();
    assert!(matches!(provider.checkpoint_wal(), Err(Error::Busy(_))));
    assert!(
        fs::metadata(format!("{}-wal", database.display()))
            .unwrap()
            .len()
            > 0
    );
    reader.execute_batch("ROLLBACK").unwrap();
    provider.checkpoint_wal().unwrap();
    assert_eq!(
        fs::metadata(format!("{}-wal", database.display()))
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn upgrade_crash_child() {
    let Some(path) = std::env::var_os("AUDIOLIB_TEST_UPGRADE_CRASH_DB") else {
        return;
    };
    let stop = std::env::var("AUDIOLIB_TEST_UPGRADE_CRASH_PHASE").unwrap();
    let _ = SqliteProvider::open_existing_with_progress(Path::new(&path), |event| {
        let reached = match event {
            SnapshotUpgradeProgress::Tokenizing {
                completed: 5000, ..
            } => stop == "tokenizing",
            SnapshotUpgradeProgress::Indexing {
                completed: 5000, ..
            } => stop == "indexing",
            SnapshotUpgradeProgress::Validating => stop == "validating",
            SnapshotUpgradeProgress::Checkpoint => stop == "checkpoint",
            _ => false,
        };
        if reached {
            std::process::exit(73);
        }
    });
    panic!("Crash phase was never reached");
}

#[test]
fn abrupt_process_exit_preserves_resume_cursor_and_all_user_data() {
    for phase in ["tokenizing", "indexing", "validating", "checkpoint"] {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("crash.db");
        legacy_fixture(&database, 5_003);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "upgrade_crash_child"])
            .env("AUDIOLIB_TEST_UPGRADE_CRASH_DB", &database)
            .env("AUDIOLIB_TEST_UPGRADE_CRASH_PHASE", phase)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(73), "{phase}: {output:?}");
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT phase FROM audiolib_search_upgrade", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            phase
        );
        drop(connection);
        let provider = SqliteProvider::open_existing(&database).unwrap();
        assert_eq!(
            provider
                .search(&SearchRequest {
                    query: "plos".into(),
                    ..Default::default()
                })
                .unwrap()
                .total_count,
            5003
        );
        assert_eq!(provider.get_audio(i64::MIN).unwrap().notes, "retain notes");
        assert_eq!(
            provider.get_audio(i64::MAX).unwrap().attributes["private"],
            "keep"
        );
        assert!(SqliteProvider::check_owned_file(&database).unwrap().is_ok());
        drop(provider);
        let mut events = Vec::new();
        drop(
            SqliteProvider::open_existing_with_progress(&database, |event| events.push(event))
                .unwrap(),
        );
        assert!(!events.iter().any(|e| matches!(
            e,
            SnapshotUpgradeProgress::BackingUp
                | SnapshotUpgradeProgress::Indexing { .. }
                | SnapshotUpgradeProgress::Tokenizing { .. }
        )));
    }
}
