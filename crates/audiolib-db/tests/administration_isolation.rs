// crates/audiolib-db/tests/administration_isolation.rs
mod support;
use audiolib_core::*;
use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn patch(tags: &str) -> MetadataPatch {
    MetadataPatch {
        tags: Some(tags.into()),
        ..Default::default()
    }
}
fn fixture() -> (tempfile::TempDir, Arc<SqliteProvider>) {
    let temp = tempfile::tempdir().unwrap();
    let provider = Arc::new(SqliteProvider::open(&temp.path().join("library.db")).unwrap());
    let conn = support::connection(provider.database_path()).unwrap();
    conn.execute_batch(
        "INSERT INTO folders(id,folder_path,folder_name) VALUES(1,'owned','owned');
         INSERT INTO folder_closure VALUES(1,1,0);
         INSERT INTO audio_files(id,file_path,path_key,filename,folder_id)
         VALUES(1,'owned.wav','owned.wav','owned.wav',1);",
    )
    .unwrap();
    provider.update_metadata(1, &patch("before")).unwrap();
    (temp, provider)
}
fn lock_path(database: &Path, suffix: &str) -> PathBuf {
    let database = database.canonicalize().unwrap();
    let mut name = database.file_name().unwrap().to_os_string();
    name.push(suffix);
    database.with_file_name(name)
}
fn lock_file(database: &Path, suffix: &str) -> fs::File {
    fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path(database, suffix))
        .unwrap()
}
fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for test barrier"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn rollback_ready(directory: &Path) -> bool {
    fs::read_dir(directory).unwrap().any(|entry| {
        let entry = entry.unwrap();
        entry
            .file_name()
            .to_string_lossy()
            .starts_with("audiolib-rollback-")
            && entry.metadata().unwrap().len() > 1024
    })
}

#[test]
fn failed_import_after_open_removes_only_its_owned_staging_sidecar() {
    let (temp, provider) = fixture();
    let source = support::connection(provider.database_path()).unwrap();
    source
        .execute_batch(
            "PRAGMA foreign_keys=OFF;
             INSERT INTO project_references(audio_file_id,project_path) VALUES(9999,'owned.rpp');",
        )
        .unwrap();
    let destination = temp.path().join("failed-import.db");
    assert!(audiolib_db::import_legacy(provider.database_path(), &destination).is_err());
    assert!(!destination.exists());
    assert!(lock_path(provider.database_path(), ".admin.lock").exists());
    assert!(!fs::read_dir(temp.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("audiolib-import-")
    }));
    assert_eq!(provider.get_audio(1).unwrap().tags, "before");
}

#[test]
fn failed_maintenance_excludes_other_instances_and_keeps_acknowledged_writes() {
    let (temp, provider) = fixture();
    let editor = SqliteProvider::open_existing(provider.database_path()).unwrap();
    editor
        .update_metadata(1, &patch("acknowledged-before"))
        .unwrap();
    editor.record_play(1).unwrap();
    let blocker = support::connection(provider.database_path()).unwrap();
    // A damaged trigger causes the final maintenance check to fail. BEGIN IMMEDIATE
    // only controls interleaving; all annotation/play writes below use the public API.
    blocker
        .execute_batch("DROP TRIGGER audio_files_au; BEGIN IMMEDIATE;")
        .unwrap();
    let worker_provider = provider.clone();
    let worker =
        thread::spawn(move || worker_provider.maintain_library(MaintenanceAction::Optimize));
    wait_for(|| rollback_ready(temp.path()));
    assert!(matches!(
        editor.update_metadata(1, &patch("must-not-be-acknowledged")),
        Err(Error::Busy(_))
    ));
    editor.record_play(1).unwrap(); // Playback only validates the existing ID.
    blocker.execute_batch("ROLLBACK;").unwrap();
    assert!(worker.join().unwrap().is_err());
    assert_eq!(provider.get_audio(1).unwrap().tags, "acknowledged-before");
    assert_eq!(provider.get_audio(1).unwrap().play_count, 0);
    assert_eq!(provider.play_history(10, 0).unwrap().len(), 0);
    editor
        .update_metadata(1, &patch("acknowledged-after"))
        .unwrap();
    editor.record_play(1).unwrap();
    assert_eq!(provider.get_audio(1).unwrap().tags, "acknowledged-after");
    assert_eq!(provider.get_audio(1).unwrap().play_count, 0);
}

#[test]
fn restore_holds_isolation_through_install_and_releases_it_afterward() {
    let (temp, provider) = fixture();
    let editor = SqliteProvider::open_existing(provider.database_path()).unwrap();
    let backup = temp.path().join("source.db");
    provider.backup_library(&backup).unwrap();
    editor.update_metadata(1, &patch("pre-restore")).unwrap();
    let blocker = support::connection(provider.database_path()).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let worker_provider = provider.clone();
    let worker = thread::spawn(move || worker_provider.restore_library(&backup));
    wait_for(|| rollback_ready(temp.path()));
    assert!(matches!(
        editor.update_metadata(1, &patch("blocked")),
        Err(Error::Busy(_))
    ));
    editor.record_play(1).unwrap(); // Playback only validates the existing ID.
    blocker.execute_batch("ROLLBACK;").unwrap();
    let report = worker.join().unwrap().unwrap();
    assert_eq!(provider.get_audio(1).unwrap().tags, "before");
    let rollback = SqliteProvider::open_read_only(&report.rollback_backup).unwrap();
    assert_eq!(rollback.get_audio(1).unwrap().tags, "pre-restore");
    editor.update_metadata(1, &patch("after-restore")).unwrap();
    editor.record_play(1).unwrap();
    assert_eq!(provider.get_audio(1).unwrap().tags, "after-restore");
    assert_eq!(provider.get_audio(1).unwrap().play_count, 0);
}

#[test]
fn exclusive_administration_lock_covers_all_writing_entry_points_and_open() {
    let (temp, provider) = fixture();
    // Require actual backfill work, so an unchecked fast path cannot make this
    // writing entry point appear safe while restore owns the administration lease.
    support::connection(provider.database_path())
        .unwrap()
        .execute_batch(
            "UPDATE audio_files SET source_tags_text='',
             metadata_json='{\"wav_metadata\":{\"fields\":{\"keywords\":\"Engine\"}}}' WHERE id=1;
             DELETE FROM library_meta WHERE key='source_tags_rev';",
        )
        .unwrap();
    let original = fs::read(provider.database_path()).unwrap();
    let mut wal_path = provider.database_path().as_os_str().to_os_string();
    wal_path.push("-wal");
    let wal_path = PathBuf::from(wal_path);
    let original_wal = fs::read(&wal_path).ok();
    let input = temp.path().join("annotations.jsonl");
    fs::write(
        &input,
        "{\"file_path\":\"owned.wav\",\"tags\":\"blocked\"}\n",
    )
    .unwrap();
    let exclusive = lock_file(provider.database_path(), ".admin.lock");
    exclusive.lock().unwrap();
    assert!(matches!(
        provider.update_metadata(1, &patch("blocked")),
        Err(Error::Busy(_))
    ));
    provider.record_play(1).unwrap(); // No write lease is needed for a no-op.
    assert!(matches!(
        provider.ensure_source_tags_current(),
        Err(Error::Busy(_))
    ));
    assert!(matches!(
        provider.update_metadata_batch(&BatchMetadataRequest {
            ids: vec![1],
            patch: patch("blocked"),
            ..Default::default()
        }),
        Err(Error::Busy(_))
    ));
    assert!(matches!(
        provider.import_annotations(&input),
        Err(Error::Busy(_))
    ));
    assert!(matches!(provider.remove_audio(&[1]), Err(Error::Busy(_))));
    assert!(matches!(provider.remove_folder(1), Err(Error::Busy(_))));
    assert!(matches!(
        SqliteProvider::open_existing(provider.database_path()),
        Err(Error::Busy(_))
    ));
    assert_eq!(provider.get_audio(1).unwrap().tags, "before");
    assert_eq!(fs::read(provider.database_path()).unwrap(), original);
    assert_eq!(fs::read(&wal_path).ok(), original_wal);
    exclusive.unlock().unwrap();
    assert_eq!(provider.ensure_source_tags_current().unwrap(), 1);
    let derived: String = support::connection(provider.database_path())
        .unwrap()
        .query_row(
            "SELECT source_tags_text FROM audio_files WHERE id=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(derived, "engine");
    // A current-schema open can coexist with normal writers; an upgrade cannot.
    let shared = lock_file(provider.database_path(), ".admin.lock");
    shared.lock_shared().unwrap();
    let reopened = SqliteProvider::open_existing(provider.database_path()).unwrap();
    reopened.update_metadata(1, &patch("shared-edit")).unwrap();
    support::connection(provider.database_path())
        .unwrap()
        .execute_batch("DELETE FROM library_meta WHERE key='source_tags_rev';")
        .unwrap();
    assert!(matches!(
        SqliteProvider::open_existing(provider.database_path()),
        Err(Error::Busy(_))
    ));
    shared.unlock().unwrap();
    assert_eq!(
        SqliteProvider::open_existing(provider.database_path())
            .unwrap()
            .library_info()
            .unwrap()
            .schema_version,
        audiolib_db::SCHEMA_VERSION
    );
}

#[test]
fn scans_allow_metadata_and_play_noops_from_another_provider() {
    let (temp, provider) = fixture();
    let editor = SqliteProvider::open_existing(provider.database_path()).unwrap();
    let root = temp.path().join("audio");
    fs::create_dir(&root).unwrap();
    let mut edited = false;
    ScanService::new(provider.clone())
        .scan(
            &root,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {
                if !edited {
                    editor.update_metadata(1, &patch("during-scan")).unwrap();
                    editor.record_play(1).unwrap();
                    edited = true;
                }
            },
        )
        .unwrap();
    assert!(edited);
    assert_eq!(provider.get_audio(1).unwrap().tags, "during-scan");
    assert_eq!(provider.get_audio(1).unwrap().play_count, 0);
}

#[test]
fn cross_process_administration_blocks_edits_then_releases_without_losing_writes() {
    let (temp, provider) = fixture();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "api_writer_child", "--nocapture"])
        .env("AUDIOLIB_DB_ISOLATION_CHILD", temp.path())
        .spawn()
        .unwrap();
    wait_for(|| temp.path().join("ready").exists());
    let exclusive = lock_file(provider.database_path(), ".admin.lock");
    exclusive.lock().unwrap();
    fs::write(temp.path().join("blocked-phase"), []).unwrap();
    wait_for(|| {
        temp.path().join("blocked-ok").exists() || temp.path().join("child-error").exists()
    });
    assert!(!temp.path().join("child-error").exists());
    assert_eq!(provider.get_audio(1).unwrap().tags, "before");
    exclusive.unlock().unwrap();
    // Keep the scan lock held: ordinary editing must still work across processes.
    let scan = lock_file(provider.database_path(), ".scan.lock");
    scan.lock().unwrap();
    fs::write(temp.path().join("released-phase"), []).unwrap();
    wait_for(|| temp.path().join("done").exists() || temp.path().join("child-error").exists());
    assert!(child.wait().unwrap().success());
    assert_eq!(
        provider.get_audio(1).unwrap().tags,
        "cross-process-acknowledged"
    );
    assert_eq!(provider.get_audio(1).unwrap().play_count, 0);
}

#[test]
fn api_writer_child() {
    let Some(directory) = std::env::var_os("AUDIOLIB_DB_ISOLATION_CHILD") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let result = (|| -> std::result::Result<(), String> {
        let provider = SqliteProvider::open_existing(&directory.join("library.db"))
            .map_err(|e| e.to_string())?;
        fs::write(directory.join("ready"), []).unwrap();
        wait_for(|| directory.join("blocked-phase").exists());
        if !matches!(
            provider.update_metadata(1, &patch("must-block")),
            Err(Error::Busy(_))
        ) || provider.record_play(1).is_err()
        {
            return Err("Writer entered the exclusive administration window".into());
        }
        fs::write(directory.join("blocked-ok"), []).unwrap();
        wait_for(|| directory.join("released-phase").exists());
        provider
            .update_metadata(1, &patch("cross-process-acknowledged"))
            .map_err(|e| e.to_string())?;
        provider.record_play(1).map_err(|e| e.to_string())?;
        fs::write(directory.join("done"), []).unwrap();
        Ok(())
    })();
    if let Err(error) = &result {
        fs::write(directory.join("child-error"), error).unwrap();
    }
    result.unwrap();
}
