// crates/audiolib-db/tests/migration_unc.rs
//! Migration attaches owned inputs read-only, including escaped local and UNC paths.
mod support;

use audiolib_core::DataProvider;
use audiolib_db::{SCHEMA_VERSION, SqliteProvider, migration};
use rusqlite::{Connection, OpenFlags};
use std::{fs, path::Path, sync::atomic::AtomicBool};

fn migrate_owned_fixture(root: &Path) {
    let directory = tempfile::Builder::new()
        .prefix("audiolib-migration-unc-")
        .tempdir_in(root)
        .unwrap();
    let escaped = directory.path().join("迁移 空格 # % &");
    fs::create_dir(&escaped).unwrap();
    let path = escaped.join("library # 中文.db");
    {
        let conn = support::connection(&path).unwrap();
        conn.execute_batch(include_str!("fixtures/schema-v5.sql"))
            .unwrap();
        conn.execute_batch(
            "INSERT INTO audio_files(id,file_path,filename,user_tags,user_notes,user_rating,
                favorite,metadata_json,play_count,last_played_at)
             VALUES(42,'owned-sample.wav','metal.wav','keep tags','retain notes',5,1,
                '{\"custom\":\"保留\"}',17,'2026-10-21T09:00:00Z');
             UPDATE audiolib_audio_id_sequence SET high_water=900;
             CREATE TABLE extension_notes(audio_id INTEGER PRIMARY KEY REFERENCES audio_files(id),note TEXT);
             INSERT INTO extension_notes VALUES(42,'extension retained');",
        )
        .unwrap();
    }

    let provider = SqliteProvider::open_existing(&path).unwrap();
    assert_eq!(
        provider.library_info().unwrap().schema_version,
        SCHEMA_VERSION
    );
    let record = provider.get_audio(42).unwrap();
    assert_eq!(record.tags, "keep tags");
    assert_eq!(record.notes, "retain notes");
    assert!(record.favorite);
    assert_eq!(record.attributes["custom"], "保留");
    assert!(provider.check_library().unwrap().is_ok());
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT user_rating FROM audio_files WHERE id=42",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        5
    );
    assert_eq!(
        conn.query_row(
            "SELECT high_water FROM audiolib_audio_id_sequence",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        900
    );
    assert_eq!(
        conn.query_row(
            "SELECT note FROM extension_notes WHERE audio_id=42",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "extension retained"
    );
    drop(conn);
    drop(provider);

    let job = migration::discover(&path).unwrap().unwrap();
    assert_eq!(job.phase, migration::MigrationPhase::Complete);
    migration::verify(&job.directory, &AtomicBool::new(false)).unwrap();
    let backup = Connection::open_with_flags(
        job.directory.join("backup.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert_eq!(
        backup
            .query_row(
                "SELECT play_count FROM audio_files WHERE id=42",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        17
    );
}

#[test]
fn escaped_local_paths_preserve_library_data_and_rollback_backup() {
    let root = tempfile::tempdir().unwrap();
    migrate_owned_fixture(root.path());
}

#[cfg(windows)]
#[test]
fn unc_paths_preserve_library_data_and_rollback_backup() {
    let Some(root) = std::env::var_os("AUDIOLIB_TEST_UNC_MIGRATION_ROOT") else {
        eprintln!(
            "UNC migration not exercised: set AUDIOLIB_TEST_UNC_MIGRATION_ROOT to an owned writable UNC directory"
        );
        return;
    };
    let root = Path::new(&root).canonicalize().unwrap();
    assert!(root.is_dir());
    assert!(
        root.to_string_lossy().starts_with(r"\\?\UNC\"),
        "UNC regression root must resolve to a network path: {}",
        root.display()
    );
    migrate_owned_fixture(&root);
}
