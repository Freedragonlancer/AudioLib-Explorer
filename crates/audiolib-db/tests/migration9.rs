// crates/audiolib-db/tests/migration9.rs
//! Online directory-only migration: retained bytes, restart, and rollback contracts.
use audiolib_core::{DataProvider, MetadataPatch, SearchRequest, SearchScope};
use audiolib_db::{
    SqliteProvider,
    migration9::{self, Action, Options, State},
};
use rusqlite::{Connection, params};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
mod support;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("library.db");
    drop(SqliteProvider::open(&db).unwrap());
    let c = support::connection(&db).unwrap();
    c.execute_batch("INSERT INTO folders(id,folder_path,folder_name,parent_id,total_file_count) VALUES (1,'root','root',NULL,4),(2,'root/ＳＦＸ','ＳＦＸ',1,1),(3,'root/other','other',1,3); INSERT INTO folder_closure VALUES(1,1,0),(2,2,0),(3,3,0),(1,2,1),(1,3,1);").unwrap();
    for id in 1..=4 {
        c.execute("INSERT INTO audio_files(id,file_path,path_key,filename,folder_id,user_notes,metadata_json) VALUES(?1,?2,?2,?2,?3,'retained', '{\"extension\":\"原文\"}')", params![id,format!("sound-{id}.wav"),if id==1 {2} else {3}]).unwrap();
    }
    (dir, db)
}
fn run(db: &Path, action: Action) -> migration9::Report {
    migration9::execute(db, action, &Options::default(), &AtomicBool::new(false))
}
fn shadow(db: &Path) -> Vec<Vec<rusqlite::types::Value>> {
    let c = Connection::open(db).unwrap();
    let mut result = vec![];
    for table in [
        "audio_search_data",
        "audio_search_idx",
        "audio_search_docsize",
        "audio_search_config",
    ] {
        let mut s = c
            .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
            .unwrap();
        let n = s.column_count();
        result.extend(
            s.query_map([], |r| (0..n).map(|i| r.get(i)).collect())
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap(),
        );
    }
    result
}

#[test]
fn directory_only_switch_reopens_and_rollback_preserves_new_annotations() {
    let (_dir, db) = fixture();
    let before = shadow(&db);
    let report = run(&db, Action::Start);
    assert_eq!(report.state, State::Done, "{report:?}");
    assert_eq!(before, shadow(&db));
    let c = Connection::open(&db).unwrap();
    migration9::validate_ready(&c).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM audio_dir_search", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(c.query_row("SELECT count(*) FROM sqlite_schema WHERE name IN ('audio_path_search','vendor_metadata','v9_checkpoint')",[],|r| r.get::<_,i64>(0)).unwrap(),0);
    assert!(
        SqliteProvider::open(&db).is_err(),
        "ordinary client must not silently become an s9 writer"
    );
    let provider =
        SqliteProvider::open_server_existing(&db, &AtomicBool::new(false), |_| {}).unwrap();
    let results = provider
        .search(&SearchRequest {
            query: "SFX".into(),
            scope: SearchScope::Path,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        results.results.iter().map(|f| f.id).collect::<Vec<_>>(),
        vec![1]
    );
    provider
        .update_metadata(
            1,
            &MetadataPatch {
                notes: Some("after publication".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let after = shadow(&db);
    assert_eq!(run(&db, Action::Rollback).state, State::RolledBack);
    assert_eq!(after, shadow(&db));
    assert_eq!(provider.get_audio(1).unwrap().notes, "after publication");
    c.execute("UPDATE folders SET folder_name='later' WHERE id=2", [])
        .unwrap();
    assert_eq!(run(&db, Action::Start).state, State::Done);
    assert_eq!(
        provider
            .search(&SearchRequest {
                query: "later".into(),
                scope: SearchScope::Path,
                ..Default::default()
            })
            .unwrap()
            .results
            .iter()
            .map(|file| file.id)
            .collect::<Vec<_>>(),
        vec![1],
        "a live reader must refresh its directory cache after re-migration"
    );
    assert!(run(&db, Action::Start).batches.is_empty());
}

#[test]
fn missing_and_unrelated_databases_are_never_initialized() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("missing.db");
    assert!(run(&db, Action::Start).error_code.is_some());
    assert!(!db.exists());
    Connection::open(&db)
        .unwrap()
        .execute_batch("CREATE TABLE unrelated(value TEXT);")
        .unwrap();
    let before = std::fs::read(&db).unwrap();
    assert!(run(&db, Action::Start).error_code.is_some());
    assert_eq!(before, std::fs::read(&db).unwrap());
}

#[cfg(feature = "test-hooks")]
#[test]
fn slow_directory_batch_reduces_the_next_batch_without_changing_main_fts() {
    let (_dir, db) = fixture();
    let before = shadow(&db);
    let mut c = support::connection(&db).unwrap();
    let tx = c.transaction().unwrap();
    // The t3 batch unit is a directory, so three batches require enough folders,
    // irrespective of how many audio files belong to each folder.
    for id in 4..=150 {
        tx.execute(
            "INSERT INTO folders(id,folder_path,folder_name,parent_id,total_file_count) VALUES(?1,?2,?3,1,0)",
            params![id, format!("root/dir-{id}"), format!("dir-{id}")],
        )
        .unwrap();
        tx.execute("INSERT INTO folder_closure VALUES(?1,?1,0),(1,?1,1)", [id])
            .unwrap();
    }
    tx.commit().unwrap();
    drop(c);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_audiolib-migrate"))
        .args(["start", "--db"])
        .arg(&db)
        .env("AUDIOLIB_MIGRATION_BATCH_ROWS", "50")
        .env("AUDIOLIB_MIGRATION_SLOW_BATCH", "2:1500")
        .output()
        .unwrap();
    let report: migration9::Report = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{report:?}");
    assert_eq!(report.state, State::Done);
    assert!(report.batches.len() >= 3, "{report:?}");
    assert!(
        report.batches[2].rows < report.batches[1].rows,
        "{report:?}"
    );
    assert_eq!(report.progress.done, 150);
    assert_eq!(before, shadow(&db));
    migration9::validate_ready(&Connection::open(&db).unwrap()).unwrap();
}

#[cfg(feature = "test-hooks")]
#[test]
fn subprocess_faults_resume_directory_batches_without_rebuilding_main() {
    for (fault, done) in [
        ("abort:batch_boundary:1", 1),
        ("abort:mid_batch:1", 0),
        ("abort:mid_batch:2", 1),
        ("abort:optimize", 3),
        ("abort:switch_tx", 3),
        ("sqlite_full:batch:2", 1),
    ] {
        let (_dir, db) = fixture();
        let before = shadow(&db);
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_audiolib-migrate"))
            .args(["start", "--db"])
            .arg(&db)
            .env("AUDIOLIB_MIGRATION_BATCH_ROWS", "1")
            .env("AUDIOLIB_MIGRATION_FAULT", fault)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{fault}");
        if fault.starts_with("abort:") {
            assert!(output.stdout.is_empty(), "{fault}");
        } else {
            let report: migration9::Report = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(report.error_code, Some(migration9::Failure::DiskFull));
        }
        assert_eq!(run(&db, Action::Status).state, State::Paused, "{fault}");
        let c = support::connection(&db).unwrap();
        assert_eq!(
            c.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            8,
            "{fault}: publication must remain atomic"
        );
        assert_eq!(
            c.query_row("SELECT done FROM v9_checkpoint", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            done,
            "{fault}"
        );
        c.execute("UPDATE folders SET folder_name='renamed' WHERE id=2", [])
            .unwrap();
        let report = run(&db, Action::Resume);
        assert_eq!(report.state, State::Done, "{fault}: {report:?}");
        assert_eq!(before, shadow(&db), "{fault}");
        assert_eq!(
            migration9::search(&db, "renamed".into(), SearchScope::Path, 100, None)
                .unwrap()
                .ids,
            Some(vec![1]),
            "{fault}"
        );
    }
}
