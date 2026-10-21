// crates/audiolib-db/tests/audio_identity.rs
#![cfg(feature = "scanner")]
//! Retired identities survive scans, process lifetimes, imports and restores.
mod support;
use audiolib_core::{DataProvider, Error, MetadataPatch};
use audiolib_db::{ScanCancellation, ScanOptions, ScanService, SqliteProvider, import_legacy};
use std::{fs, path::Path, sync::Arc};

fn wav(root: &Path, name: &str) -> std::path::PathBuf {
    fs::create_dir_all(root).unwrap();
    let path = root.join(name);
    let mut bytes = b"RIFF".to_vec();
    bytes.extend(38_u32.to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(48_000_u32.to_le_bytes());
    bytes.extend(96_000_u32.to_le_bytes());
    bytes.extend(2_u16.to_le_bytes());
    bytes.extend(16_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(2_u32.to_le_bytes());
    bytes.extend(0_i16.to_le_bytes());
    fs::write(&path, bytes).unwrap();
    path
}

fn scan(provider: &Arc<SqliteProvider>, path: &Path) -> i64 {
    ScanService::new(provider.clone())
        .scan(
            path.parent().unwrap(),
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    provider.find_audio_by_path(path).unwrap().unwrap().id
}

#[test]
fn removed_maximum_id_cannot_select_or_edit_a_different_wav_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("library.db");
    let a = wav(&temp.path().join("a"), "a.wav");
    let b = wav(&temp.path().join("b"), "b.wav");
    let a_bytes = fs::read(&a).unwrap();
    let b_bytes = fs::read(&b).unwrap();
    let provider = Arc::new(SqliteProvider::open(&database).unwrap());
    let retired = scan(&provider, &a);
    assert_eq!(retired, 1); // A fresh empty library still starts at 1.
    provider.remove_audio(&[retired]).unwrap();
    drop(provider);

    let provider = Arc::new(SqliteProvider::open_existing(&database).unwrap());
    let next = scan(&provider, &b);
    assert!(next > retired);
    assert!(matches!(
        provider.get_audio(retired),
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        provider.update_metadata(
            retired,
            &MetadataPatch {
                tags: Some("stale-a-draft".into()),
                ..Default::default()
            },
        ),
        Err(Error::NotFound(_))
    ));
    assert!(provider.get_audio(next).unwrap().tags.is_empty());
    assert_eq!(fs::read(&a).unwrap(), a_bytes);
    assert_eq!(fs::read(&b).unwrap(), b_bytes);

    // The removed source still exists. Adding it again is another new identity.
    assert!(scan(&provider, &a) > next);
}

#[test]
fn schema_three_upgrade_keeps_unknown_data_and_starts_after_existing_ids() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("old-v3.db");
    let conn = support::connection(&database).unwrap();
    // Captured v5 layout with the stable-ID extension removed exercises older feature state.
    conn.execute_batch(include_str!("fixtures/schema-v5.sql"))
        .unwrap();
    conn.execute_batch(
        "ALTER TABLE audio_files ADD COLUMN private_attribute BLOB;
         CREATE TABLE private_plugin_state(value TEXT);
         INSERT INTO private_plugin_state VALUES('preserve-me');
         INSERT INTO audio_files(id,file_path,filename,private_attribute,user_tags,user_notes,play_count)
         VALUES(42,'old.wav','old.wav',X'0123','keep-tag','keep-note',7);
         DROP TRIGGER audiolib_audio_id_high_water;
         DROP TABLE audiolib_audio_id_sequence;
         DELETE FROM audiolib_schema_features WHERE feature='stable_audio_ids';
         PRAGMA user_version=3;",
    ).unwrap();
    drop(conn);

    let provider = Arc::new(SqliteProvider::open_existing(&database).unwrap());
    assert_eq!(
        provider.library_info().unwrap().schema_version,
        audiolib_db::SCHEMA_VERSION
    );
    let old = provider.get_audio(42).unwrap();
    assert_eq!(
        (&*old.tags, &*old.notes, old.play_count),
        ("keep-tag", "keep-note", 0)
    );
    let conn = support::connection(&database).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT private_attribute FROM audio_files WHERE id=42",
            [],
            |row| row.get::<_, Vec<u8>>(0)
        )
        .unwrap(),
        vec![1, 35]
    );
    assert_eq!(
        conn.query_row("SELECT value FROM private_plugin_state", [], |row| row
            .get::<_, String>(
            0
        ))
        .unwrap(),
        "preserve-me"
    );
    drop(conn);
    provider.remove_audio(&[42]).unwrap();
    let b = wav(&temp.path().join("b"), "b.wav");
    assert!(scan(&provider, &b) > 42);
}

#[test]
fn both_provider_instances_allocate_after_the_same_persistent_high_water() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("library.db");
    let first = Arc::new(SqliteProvider::open(&database).unwrap());
    let second = Arc::new(SqliteProvider::open_existing(&database).unwrap());
    let a = wav(&temp.path().join("a"), "a.wav");
    let b = wav(&temp.path().join("b"), "b.wav");
    let retired = scan(&first, &a);
    second.remove_audio(&[retired]).unwrap();
    assert!(scan(&second, &b) > retired);
}

#[test]
fn explicit_sql_insert_tracks_a_retired_high_id_across_reopening() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("library.db");
    let provider = SqliteProvider::open(&database).unwrap();
    support::connection(&database)
        .unwrap()
        .execute(
            "INSERT INTO audio_files(id,file_path,filename) VALUES(?1,'external.wav','external.wav')",
            [i64::MAX - 10_000],
        )
        .unwrap();
    provider.remove_audio(&[i64::MAX - 10_000]).unwrap();
    drop(provider);
    let provider = Arc::new(SqliteProvider::open_existing(&database).unwrap());
    let b = wav(&temp.path().join("b"), "b.wav");
    assert!(scan(&provider, &b) > i64::MAX - 10_000);
}

#[test]
fn legacy_autoincrement_import_keeps_the_sequence_of_deleted_high_ids() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("legacy.db");
    let conn = support::connection(&source).unwrap();
    conn.execute_batch(
        "CREATE TABLE audio_files(id INTEGER PRIMARY KEY AUTOINCREMENT,
         file_path TEXT NOT NULL UNIQUE,filename TEXT NOT NULL);
         INSERT INTO audio_files(file_path,filename) VALUES('old.wav','old.wav');
         DELETE FROM audio_files;",
    )
    .unwrap();
    let retired = i64::MAX - 2;
    conn.execute(
        "UPDATE sqlite_sequence SET seq=?1 WHERE name='audio_files'",
        [retired],
    )
    .unwrap();
    drop(conn);
    let source_bytes = fs::read(&source).unwrap();
    let target = temp.path().join("imported.db");
    import_legacy(&source, &target).unwrap();
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    let provider = Arc::new(SqliteProvider::open_existing(&target).unwrap());
    let b = wav(&temp.path().join("b"), "b.wav");
    assert!(scan(&provider, &b) > retired);
}

#[test]
fn an_imported_copy_keeps_retired_ids_even_when_the_source_has_no_records() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.db");
    let provider = Arc::new(SqliteProvider::open(&source).unwrap());
    let a = wav(&temp.path().join("a"), "a.wav");
    let retired = scan(&provider, &a);
    provider.remove_audio(&[retired]).unwrap();
    drop(provider);
    let before = fs::read(&source).unwrap();
    let copy = temp.path().join("copy.db");
    import_legacy(&source, &copy).unwrap();
    assert_eq!(fs::read(&source).unwrap(), before);
    let provider = Arc::new(SqliteProvider::open_existing(&copy).unwrap());
    let b = wav(&temp.path().join("b"), "b.wav");
    assert!(scan(&provider, &b) > retired);
}

#[test]
fn restoring_an_older_backup_preserves_the_active_librarys_retired_ids() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("library.db");
    let provider = Arc::new(SqliteProvider::open(&database).unwrap());
    let a = wav(&temp.path().join("a"), "a.wav");
    let a_id = scan(&provider, &a);
    provider
        .update_metadata(
            a_id,
            &MetadataPatch {
                tags: Some("backup-tag".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let backup = temp.path().join("backup.db");
    provider.backup_library(&backup).unwrap();
    let backup_bytes = fs::read(&backup).unwrap();
    let b = wav(&temp.path().join("b"), "b.wav");
    let retired = scan(&provider, &b);
    provider.remove_audio(&[retired]).unwrap();
    provider.restore_library(&backup).unwrap();
    assert_eq!(provider.get_audio(a_id).unwrap().tags, "backup-tag");
    assert_eq!(fs::read(&backup).unwrap(), backup_bytes);
    drop(provider);

    let provider = Arc::new(SqliteProvider::open_existing(&database).unwrap());
    let c = wav(&temp.path().join("c"), "c.wav");
    assert!(scan(&provider, &c) > retired);
    assert!(matches!(
        provider.get_audio(retired),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn identity_exhaustion_fails_without_wrapping_or_modifying_source_audio() {
    let temp = tempfile::tempdir().unwrap();
    let provider = Arc::new(SqliteProvider::open(&temp.path().join("library.db")).unwrap());
    support::connection(provider.database_path())
        .unwrap()
        .execute(
            "UPDATE audiolib_audio_id_sequence SET high_water=?1 WHERE singleton=1",
            [i64::MAX],
        )
        .unwrap();
    let source = wav(&temp.path().join("audio"), "new.wav");
    let before = fs::read(&source).unwrap();
    let result = ScanService::new(provider.clone()).scan(
        source.parent().unwrap(),
        &ScanOptions::default(),
        &ScanCancellation::default(),
        |_| {},
    );
    assert!(result.is_err());
    assert_eq!(provider.stats().unwrap().audio_files, 0);
    assert_eq!(fs::read(&source).unwrap(), before);
}
