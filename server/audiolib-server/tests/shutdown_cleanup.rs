// server/audiolib-server/tests/shutdown_cleanup.rs
//! Graceful shutdown reclaims owned caches and WAL without inventing a content revision.
use audiolib_db::{SnapshotUpgradeProgress, SqliteProvider};
use audiolib_server::{AppState, ServerConfig, Service, state::remove_stale_snapshots};
use rusqlite::Connection;
use std::{fs, time::Duration};

fn fixture() -> (tempfile::TempDir, ServerConfig) {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("library.db");
    drop(SqliteProvider::open(&database).unwrap());
    let conn = Connection::open(&database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute(
        "INSERT INTO audio_files(id,file_path,filename) VALUES(1,'fixture.wav','fixture.wav')",
        [],
    )
    .unwrap();
    drop(conn);
    let config = ServerConfig::from_toml_str(
        "library_db='library.db'\nsnapshot_dir='snapshots'",
        dir.path(),
    )
    .unwrap();
    (dir, config)
}

#[test]
fn shutdown_releases_snapshot_and_wal_and_preserves_restart_revision() {
    let (dir, config) = fixture();
    let state = AppState::open(&config, None).unwrap();
    let conn = Connection::open(&config.library_db).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute("UPDATE audio_files SET favorite=1", [])
        .unwrap();
    let revision = state.poll_revision().unwrap();
    let snapshot = state.prepare_snapshot().unwrap();
    let plain = snapshot.plain.path.clone();
    let compressed = snapshot.zstd_file().unwrap();
    let packed = compressed.path.clone();
    // Another instance's cleaner must not delete files belonging to active readers.
    remove_stale_snapshots(&dir.path().join("snapshots"), None);
    assert!(plain.is_file());
    assert!(packed.is_file());
    drop(compressed);
    drop(snapshot);
    assert_eq!(
        state.poll_revision().unwrap(),
        revision,
        "snapshot creation must not change the content revision"
    );
    state.shutdown(Duration::from_secs(5)).unwrap();
    assert!(!plain.exists());
    assert!(!packed.exists());
    assert_eq!(state.revision().unwrap(), revision);
    let wal = format!("{}-wal", config.library_db.display());
    assert_eq!(fs::metadata(&wal).map(|m| m.len()).unwrap_or(0), 0);
    assert!(state.prepare_snapshot().is_err());
    drop(conn);
    drop(state);
    let reopened = AppState::open(&config, None).unwrap();
    assert_eq!(reopened.revision().unwrap(), revision);
    reopened.shutdown(Duration::from_secs(5)).unwrap();
    drop(reopened);
    // A real external commit while stopped must still advance the revision.
    let conn = Connection::open(&config.library_db).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute("UPDATE audio_files SET favorite=0", [])
        .unwrap();
    drop(conn);
    let reopened = AppState::open(&config, None).unwrap();
    assert!(reopened.revision().unwrap().revision > revision.revision);
}

#[test]
fn active_download_retains_its_owned_files_until_the_last_reference_drops() {
    let (_dir, config) = fixture();
    let state = AppState::open(&config, None).unwrap();
    let response = state.prepare_snapshot().unwrap();
    let path = response.plain.path.clone();
    state.shutdown(Duration::from_secs(5)).unwrap();
    assert!(path.exists());
    remove_stale_snapshots(path.parent().unwrap().parent().unwrap(), None);
    assert!(path.exists());
    drop(response);
    assert!(!path.exists());
}

#[test]
fn stale_cleaner_requires_a_released_owner_lock() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = dir.path().join("audiolib-snapshot-legacy");
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join("library.db"), "retain").unwrap();
    let stale = dir.path().join("audiolib-snapshot-crashed");
    fs::create_dir(&stale).unwrap();
    fs::write(stale.join(".owner.lock"), "").unwrap();
    fs::write(stale.join("library.db"), "owned").unwrap();
    remove_stale_snapshots(dir.path(), None);
    assert!(legacy.exists());
    assert!(!stale.exists());
}

#[tokio::test]
async fn health_exposes_only_phase_and_committed_counts() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let service = Service::starting(None);
    service.set_upgrade_progress(SnapshotUpgradeProgress::Indexing {
        completed: 5000,
        total: 900000,
    });
    let response = audiolib_server::router(service)
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = to_bytes(response.into_body(), 16_384).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "starting");
    assert_eq!(
        json["upgrade"],
        serde_json::json!({"phase":"indexing","completed":5000,"total":900000})
    );
}

#[tokio::test]
async fn shutdown_drains_admitted_background_snapshot_and_prevents_repopulation() {
    let (dir, mut config) = fixture();
    config.snapshot_wait_secs = 0;
    let state = AppState::open(&config, None).unwrap();
    state.set_snapshot_prepare_delay(Duration::from_millis(150));
    assert!(state.snapshot_within_budget(true).await.is_none());
    let stopping = state.clone();
    tokio::task::spawn_blocking(move || stopping.shutdown(Duration::from_secs(5)))
        .await
        .unwrap()
        .unwrap();
    assert!(state.snapshot_within_budget(true).await.unwrap().is_err());
    let count = fs::read_dir(dir.path().join("snapshots"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(count, 0);
}

#[tokio::test]
async fn shutdown_cancels_an_active_large_snapshot_and_removes_its_staging_files() {
    let (dir, mut config) = fixture();
    // A large unrelated payload exercises bounded backup/compression without adding
    // artificial FTS work or touching any real library.
    let conn = Connection::open(&config.library_db).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE snapshot_payload(bytes BLOB);
         INSERT INTO snapshot_payload VALUES(zeroblob(134217728));",
    )
    .unwrap();
    drop(conn);
    config.snapshot_wait_secs = 0;
    let state = AppState::open(&config, None).unwrap();
    assert!(state.snapshot_within_budget(true).await.is_none());
    let snapshots = dir.path().join("snapshots");
    let started = std::time::Instant::now();
    loop {
        // A nonempty staging payload proves the job passed admission and started I/O.
        let has_payload = fs::read_dir(&snapshots).ok().is_some_and(|entries| {
            entries.flatten().any(|entry| {
                fs::read_dir(entry.path()).ok().is_some_and(|files| {
                    files
                        .flatten()
                        .any(|file| file.metadata().is_ok_and(|metadata| metadata.len() > 0))
                })
            })
        });
        if has_payload {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let stopping = state.clone();
    tokio::task::spawn_blocking(move || stopping.shutdown(Duration::from_secs(5)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fs::read_dir(snapshots).unwrap().count(), 0);
    assert!(state.prepare_snapshot().is_err());
}
