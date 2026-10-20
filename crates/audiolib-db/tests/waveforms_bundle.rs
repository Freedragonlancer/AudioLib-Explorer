// crates/audiolib-db/tests/waveforms_bundle.rs
//! `waveforms.db` bundle validation and atomic replacement (doc/api/README.md §9.4.2).
use audiolib_core::{LIST_WAVEFORM_ENCODING_I8, ListWaveformI8};
use audiolib_db::{WAVEFORMS_DB_FILE, WaveformsDb, WaveformsDbRow, validate_bundle};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

const LIBRARY: &str = "lib-bundle";
const SCHEMA: &str = "CREATE TABLE list_waveforms (file_id INTEGER PRIMARY KEY, etag TEXT NOT NULL,
  src_mtime INTEGER NOT NULL, src_size INTEGER NOT NULL, encoding TEXT NOT NULL,
  n INTEGER NOT NULL, blob BLOB NOT NULL);
  CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

/// A server-style bundle with two rows; `tweak` runs before the file is closed.
fn bundle(directory: &Path, name: &str, tweak: &str) -> PathBuf {
    let path = directory.join(name);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    let mut blob = vec![0u8; 256];
    blob[0] = (-5i8) as u8;
    blob[1] = 5;
    conn.execute(
        "INSERT INTO list_waveforms VALUES(1,'W/\"a-l1i8\"',0,0,?1,1,?2)",
        rusqlite::params![LIST_WAVEFORM_ENCODING_I8, blob],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO list_waveforms VALUES(2,'W/\"b-l1i8\"',0,0,?1,0,zeroblob(256))",
        [LIST_WAVEFORM_ENCODING_I8],
    )
    .unwrap();
    conn.execute_batch(&format!(
        "INSERT INTO meta VALUES('library_id','{LIBRARY}'),('revision','7'),
         ('encoding','{LIST_WAVEFORM_ENCODING_I8}'),('schema_version','1'),('generated_at','1');"
    ))
    .unwrap();
    if !tweak.is_empty() {
        conn.execute_batch(tweak).unwrap();
    }
    drop(conn);
    path
}

fn rejected(directory: &Path, name: &str, tweak: &str) -> String {
    let path = bundle(directory, name, tweak);
    validate_bundle(&path, LIST_WAVEFORM_ENCODING_I8, LIBRARY)
        .expect_err(name)
        .to_string()
}

#[test]
fn a_valid_bundle_passes() {
    let directory = tempfile::tempdir().unwrap();
    let path = bundle(directory.path(), "ok.db", "");
    assert_eq!(
        validate_bundle(&path, LIST_WAVEFORM_ENCODING_I8, LIBRARY).unwrap(),
        2
    );
}

#[test]
fn every_validation_failure_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let d = directory.path();
    let cases = [
        (
            "extra_table.db",
            "CREATE TABLE extra(x);",
            "unexpected tables",
        ),
        ("view.db", "CREATE VIEW v AS SELECT 1;", "unexpected view"),
        (
            "trigger.db",
            "CREATE TRIGGER t AFTER INSERT ON meta BEGIN SELECT 1; END;",
            "unexpected trigger",
        ),
        (
            "extra_index.db",
            "CREATE INDEX i ON list_waveforms(etag);",
            "unexpected index",
        ),
        (
            "extra_column.db",
            "ALTER TABLE list_waveforms ADD COLUMN extra TEXT;",
            "list_waveforms columns",
        ),
        (
            "missing_column.db",
            "ALTER TABLE list_waveforms DROP COLUMN src_size;",
            "list_waveforms columns",
        ),
        (
            "meta_columns.db",
            "ALTER TABLE meta ADD COLUMN extra TEXT;",
            "meta columns",
        ),
        (
            "short_blob.db",
            "UPDATE list_waveforms SET blob=zeroblob(255) WHERE file_id=1;",
            "bad blob",
        ),
        (
            "long_blob.db",
            "UPDATE list_waveforms SET blob=zeroblob(257) WHERE file_id=1;",
            "bad blob",
        ),
        (
            "text_blob.db",
            "UPDATE list_waveforms SET blob=printf('%256s','x') WHERE file_id=1;",
            "bad blob",
        ),
        (
            "encoding.db",
            "UPDATE list_waveforms SET encoding='i16le-minmax-base64' WHERE file_id=2;",
            "bad blob",
        ),
        (
            "n_high.db",
            "UPDATE list_waveforms SET n=129 WHERE file_id=2;",
            "bad blob",
        ),
        (
            "n_low.db",
            "UPDATE list_waveforms SET n=-1 WHERE file_id=2;",
            "bad blob",
        ),
        (
            "other_library.db",
            "UPDATE meta SET value='other' WHERE key='library_id';",
            "meta.library_id",
        ),
        (
            "no_library.db",
            "DELETE FROM meta WHERE key='library_id';",
            "meta.library_id",
        ),
        ("no_meta_table.db", "DROP TABLE meta;", "unexpected tables"),
    ];
    for (name, tweak, expected) in cases {
        let message = rejected(d, name, tweak);
        assert!(message.contains(expected), "{name}: {message}");
    }
}

#[test]
fn corrupt_and_non_sqlite_files_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let garbage = directory.path().join("garbage.db");
    std::fs::write(&garbage, b"not a database at all, just bytes").unwrap();
    assert!(validate_bundle(&garbage, LIST_WAVEFORM_ENCODING_I8, LIBRARY).is_err());
    // Damage the table pages of an otherwise valid bundle.
    let path = bundle(directory.path(), "damaged.db", "");
    let mut bytes = std::fs::read(&path).unwrap();
    let page = 4096.min(bytes.len() / 2);
    for byte in &mut bytes[page..] {
        *byte = 0xA5;
    }
    std::fs::write(&path, bytes).unwrap();
    assert!(validate_bundle(&path, LIST_WAVEFORM_ENCODING_I8, LIBRARY).is_err());
}

#[test]
fn a_validated_bundle_replaces_the_store_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let store = WaveformsDb::open(&directory.path().join(WAVEFORMS_DB_FILE)).unwrap();
    store
        .upsert(&WaveformsDbRow {
            file_id: 9,
            etag: "W/\"old-l1i8\"".into(),
            encoding: LIST_WAVEFORM_ENCODING_I8.into(),
            src_mtime: 0,
            src_size: 0,
            peaks: ListWaveformI8::new(vec![-1, 1]).unwrap(),
        })
        .unwrap();
    let staged = bundle(directory.path(), "waveforms.db.bundle.tmp", "");
    validate_bundle(&staged, LIST_WAVEFORM_ENCODING_I8, LIBRARY).unwrap();
    store.replace_with(&staged).unwrap();
    assert!(!staged.exists());
    // Rows already in the store are newer than the bundle's and are kept (S6).
    assert_eq!(store.get(9).unwrap().unwrap().etag, "W/\"old-l1i8\"");
    let one = store.get(1).unwrap().unwrap();
    assert_eq!(one.etag, "W/\"a-l1i8\"");
    assert_eq!(one.peaks.values, vec![-5, 5]);
    assert_eq!(store.len().unwrap(), 3);
    // Still writable in WAL mode after the swap.
    store
        .upsert(&WaveformsDbRow {
            file_id: 3,
            ..one.clone()
        })
        .unwrap();
    let reopened = WaveformsDb::open(store.path()).unwrap();
    assert_eq!(reopened.len().unwrap(), 4);
    // A bundle staged elsewhere is refused and the store stays intact.
    let elsewhere = tempfile::tempdir().unwrap();
    let foreign = bundle(elsewhere.path(), "b.db", "");
    assert!(store.replace_with(&foreign).is_err());
    assert_eq!(store.len().unwrap(), 4);
}
