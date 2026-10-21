// crates/audiolib-db/src/migration/local_history_tests.rs
//! Cross-store ordering and recovery tests for explicitly local upgrades.
use super::*;
use crate::{
    DataProvider, SqliteProvider,
    local_play_history::{LocalPlayHistory, local_library_id},
};
use rusqlite::Connection;
use std::sync::atomic::AtomicBool;

fn old_library() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("library.db");
    let data = root.path().join("client-data");
    std::fs::create_dir(&data).unwrap();
    let conn = Connection::open(&path).unwrap();
    crate::register_library_functions(&conn).unwrap();
    conn.execute_batch(include_str!("../../tests/fixtures/schema-v5.sql"))
        .unwrap();
    conn.execute_batch(
        "INSERT INTO audio_files(id,file_path,filename,user_notes,play_count,last_played_at)
        VALUES(1,'owned/one.wav','one.wav','retained notes',17,'2026-10-20T00:00:00Z');",
    )
    .unwrap();
    for id in 1..=6 {
        conn.execute("INSERT INTO play_history(id,audio_file_id,played_at,filename,file_path) VALUES(?1,1,?2,'one.wav','owned/one.wav')",
            rusqlite::params![id, format!("2026-10-20T00:00:0{id}.000Z")]).unwrap();
    }
    drop(conn);
    (root, path, data)
}

#[test]
fn local_import_failure_is_before_source_guards_and_resumes_from_verified_backup() {
    let (_root, path, data) = old_library();
    let cancel = AtomicBool::new(false);
    let original = std::fs::read(&path).unwrap();
    let reviewed = plan_local(&path, &data, 3, &cancel).unwrap();
    let obstacle = data.join("play-history");
    std::fs::write(&obstacle, b"owned blocking file").unwrap();
    assert!(apply(&reviewed, &cancel, &mut |_| Ok(())).is_err());
    let job = discover(&path).unwrap().unwrap();
    assert_eq!(job.phase, MigrationPhase::Backup);
    assert!(job.backup_sha256.is_some());
    assert!(job.directory.join("backup.db").is_file());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let source = Connection::open(&path).unwrap();
    assert_eq!(
        source
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='audiolib_storage_migration'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    drop(source);
    std::fs::remove_file(&obstacle).unwrap();
    // The server/snapshot entry point cannot take over a client-local pending job.
    assert!(SqliteProvider::open_existing(&path).is_err());
    let provider = SqliteProvider::open_existing_local_with_progress_cancellable(
        &path,
        &data,
        3,
        &cancel,
        |_| {},
    )
    .unwrap();
    assert_eq!(provider.get_audio(1).unwrap().notes, "retained notes");
    let id = local_library_id(&path).unwrap();
    let history = LocalPlayHistory::open(&data, &id, 3).unwrap();
    let rows = history.page(10, 0).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].played_at, "2026-10-20T00:00:06.000Z");
    let complete = discover(&path).unwrap().unwrap();
    assert_eq!(complete.phase, MigrationPhase::Complete);
    verify(&complete.directory, &cancel).unwrap();
    drop(provider);
    rollback(&complete.directory, &cancel, &mut |_| Ok(())).unwrap();
    let original_conn = Connection::open(&path).unwrap();
    assert_eq!(
        original_conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        original_conn
            .query_row("SELECT play_count FROM audio_files", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        17
    );
    assert_eq!(
        original_conn
            .query_row("SELECT COUNT(*) FROM play_history", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        original_conn
            .query_row("SELECT last_played_at FROM audio_files", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "2026-10-20T00:00:00Z"
    );
    drop(original_conn);
    // A deliberate re-upgrade uses a different backup binding but never duplicates events.
    let next = plan_local(&path, &data, 3, &cancel).unwrap();
    apply(&next, &cancel, &mut |_| Ok(())).unwrap();
    assert_eq!(history.page(10, 0).unwrap(), rows);
}

#[test]
fn server_and_snapshot_jobs_never_import_history_and_local_zero_is_disabled() {
    for local in [false, true] {
        let (_root, path, data) = old_library();
        let cancel = AtomicBool::new(false);
        let reviewed = if local {
            plan_local(&path, &data, 0, &cancel)
        } else {
            plan(&path, &cancel)
        }
        .unwrap();
        let done = apply(&reviewed, &cancel, &mut |_| Ok(())).unwrap();
        verify(&done.directory, &cancel).unwrap();
        if local {
            let id = local_library_id(&path).unwrap();
            let history = LocalPlayHistory::open(&data, &id, 0).unwrap();
            assert!(history.page(20, 0).unwrap().is_empty());
            assert!(
                history
                    .record(Some(1), "owned/one.wav", "one.wav")
                    .unwrap()
                    .is_none()
            );
        } else {
            assert!(!data.join("play-history").exists());
        }
    }
}

#[test]
fn invalid_local_event_prevents_retirement_but_a_service_can_preserve_it_in_backup() {
    let (_root, path, data) = old_library();
    let conn = Connection::open(&path).unwrap();
    conn.execute("UPDATE play_history SET played_at='invalid' WHERE id=6", [])
        .unwrap();
    drop(conn);
    let original = std::fs::read(&path).unwrap();
    let cancel = AtomicBool::new(false);
    let reviewed = plan_local(&path, &data, 5000, &cancel).unwrap();
    assert!(apply(&reviewed, &cancel, &mut |_| Ok(())).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(
        discover(&path).unwrap().unwrap().phase,
        MigrationPhase::Backup
    );
    let id = local_library_id(&path).unwrap();
    assert!(
        LocalPlayHistory::open(&data, &id, 5000)
            .unwrap()
            .page(20, 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn local_plan_binds_real_data_root_and_rejects_changed_root_before_source_mutation() {
    let (_root, path, data) = old_library();
    let cancel = AtomicBool::new(false);
    let reviewed = plan_local(&path, &data.join("."), 3, &cancel).unwrap();
    assert_eq!(
        reviewed.local_history.as_ref().unwrap().data_root,
        data.canonicalize().unwrap()
    );
    let original = std::fs::read(&path).unwrap();
    // The reviewed root must still exist, even though its lexical name is unchanged.
    std::fs::remove_dir(&data).unwrap();
    assert!(apply(&reviewed, &cancel, &mut |_| Ok(())).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(discover(&path).unwrap().is_none());
    assert!(plan_local(&path, &data, 3, &cancel).is_err());
    assert!(!data.exists());
}

#[test]
fn completed_local_job_can_verify_and_rollback_when_history_storage_is_unavailable() {
    let (root, path, data) = old_library();
    let cancel = AtomicBool::new(false);
    let reviewed = plan_local(&path, &data, 5000, &cancel).unwrap();
    let job = apply(&reviewed, &cancel, &mut |_| Ok(())).unwrap();
    let moved = root.path().join("history-temporarily-unavailable");
    std::fs::rename(&data, &moved).unwrap();
    verify(&job.directory, &cancel).unwrap();
    rollback(&job.directory, &cancel, &mut |_| Ok(())).unwrap();
    assert!(!data.exists());
    let restored = Connection::open(&path).unwrap();
    assert_eq!(
        restored
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        restored
            .query_row("SELECT COUNT(*) FROM play_history", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        6
    );
    drop(restored);
    verify(&job.directory, &cancel).unwrap();
    let id = local_library_id(&path).unwrap();
    assert_eq!(
        LocalPlayHistory::open(&moved, &id, 5000)
            .unwrap()
            .page(20, 0)
            .unwrap()
            .len(),
        6
    );
}
