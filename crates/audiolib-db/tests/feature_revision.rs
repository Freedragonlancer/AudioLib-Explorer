// crates/audiolib-db/tests/feature_revision.rs
//! Additive implementation revisions never downgrade a future library or snapshot.
use audiolib_core::{DataProvider, Error, SearchRequest};
use audiolib_db::SqliteProvider;
use rusqlite::Connection;
use std::{fs, path::Path};

fn fixture(path: &Path) {
    let provider = SqliteProvider::open(path).unwrap();
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "INSERT INTO audio_files(id,file_path,filename,filename_tokenized,
         user_tags,user_tags_tokenized,user_notes,user_notes_tokenized,user_rating,favorite,metadata_json)
         VALUES(42,'N:/fixture/café.wav','café.wav','café wav','retain tags','retain tags',
         'private notes','private notes',5,1,'{\"private\":\"keep\"}');",
    )
    .unwrap();
    drop(conn);
    drop(provider);
    // Capture a self-contained database; no committed changes remain in WAL sidecars.
    let conn = Connection::open(path).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
}

fn assert_future_error<T>(result: audiolib_core::Result<T>, feature: &str) {
    match result {
        Err(Error::Database(message)) => {
            assert!(message.contains(feature), "{message}");
            assert!(message.contains("newer than supported"), "{message}");
        }
        Err(error) => panic!("Unexpected error for {feature}: {error}"),
        Ok(_) => panic!("Future {feature} revision was accepted"),
    }
}

fn assert_business_data(provider: &SqliteProvider) {
    let file = provider.get_audio(42).unwrap();
    assert_eq!(file.filename, "café.wav");
    assert_eq!(file.tags, "retain tags");
    assert_eq!(file.notes, "private notes");
    let legacy_rating: i64 = Connection::open(provider.database_path())
        .unwrap()
        .query_row(
            "SELECT user_rating FROM audio_files WHERE id=42",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        legacy_rating, 5,
        "derived upgrades preserve stored legacy ratings"
    );
    assert!(file.favorite);
    assert_eq!(file.attributes["private"], "keep");
    let page = provider
        .search(&SearchRequest {
            query: "café".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.total_count, 1);
    assert_eq!(page.results[0].id, 42);
}

#[test]
fn future_known_revisions_are_rejected_without_changing_source_bytes() {
    let temp = tempfile::tempdir().unwrap();
    for feature in ["stable_audio_ids", "foreign_key_indexes", "tokenization"] {
        for old_tokenization in [false, true] {
            if feature == "tokenization" && old_tokenization {
                continue;
            }
            let database = temp.path().join(format!("{feature}-{old_tokenization}.db"));
            fixture(&database);
            let mut replacement = {
                let provider = SqliteProvider::open_read_only(&database).unwrap();
                provider.get_audio(42).unwrap()
            };
            replacement.tags = "new café tags".into();
            replacement.notes = "must not overwrite future annotations".into();
            replacement.favorite = false;
            let conn = Connection::open(&database).unwrap();
            conn.execute(
                "UPDATE audiolib_schema_features SET version=2 WHERE feature=?1",
                [feature],
            )
            .unwrap();
            if old_tokenization {
                conn.execute(
                    "DELETE FROM audiolib_schema_features WHERE feature='tokenization'",
                    [],
                )
                .unwrap();
            }
            drop(conn);
            let original = fs::read(&database).unwrap();

            assert_future_error(SqliteProvider::open_existing(&database), feature);
            assert_eq!(fs::read(&database).unwrap(), original);
            assert_future_error(SqliteProvider::upgrade_owned_snapshot(&database), feature);
            assert_eq!(fs::read(&database).unwrap(), original);
            assert_future_error(SqliteProvider::has_current_search_text(&database), feature);
            assert_future_error(SqliteProvider::has_current_search_state(&database), feature);
            assert_eq!(fs::read(&database).unwrap(), original);
            // Covers both the current-token fast path and the older-token copy path.
            assert_future_error(SqliteProvider::open_read_only(&database), feature);
            assert_eq!(fs::read(&database).unwrap(), original);
            assert_future_error(
                SqliteProvider::apply_snapshot_records(&database, &[replacement]),
                feature,
            );
            assert_eq!(fs::read(&database).unwrap(), original);
        }
    }
}

#[test]
fn current_revisions_keep_owned_and_readonly_snapshots_compatible() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("current.db");
    fixture(&database);
    let original = fs::read(&database).unwrap();
    assert!(SqliteProvider::has_current_search_text(&database).unwrap());
    assert!(!SqliteProvider::upgrade_owned_snapshot(&database).unwrap());
    assert_eq!(fs::read(&database).unwrap(), original);
    let readonly = SqliteProvider::open_read_only(&database).unwrap();
    assert_eq!(readonly.database_path(), database.as_path());
    assert_business_data(&readonly);
    drop(readonly);
    assert_eq!(fs::read(&database).unwrap(), original);
    let existing = SqliteProvider::open_existing(&database).unwrap();
    assert_business_data(&existing);
}

#[test]
fn absent_revisions_still_upgrade_owned_copies_and_existing_libraries() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("old-v3.db");
    fixture(&database);
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch("DROP TABLE audiolib_schema_features;")
        .unwrap();
    drop(conn);
    let original = fs::read(&database).unwrap();
    assert!(!SqliteProvider::has_current_search_text(&database).unwrap());
    let readonly = SqliteProvider::open_read_only(&database).unwrap();
    assert_ne!(readonly.database_path(), database.as_path());
    assert_business_data(&readonly);
    drop(readonly);
    assert_eq!(fs::read(&database).unwrap(), original);

    let owned = temp.path().join("owned.db");
    fs::copy(&database, &owned).unwrap();
    assert!(SqliteProvider::upgrade_owned_snapshot(&owned).unwrap());
    assert!(!SqliteProvider::upgrade_owned_snapshot(&owned).unwrap());
    assert!(SqliteProvider::has_current_search_text(&owned).unwrap());
    let readonly = SqliteProvider::open_read_only(&owned).unwrap();
    assert_business_data(&readonly);
    assert_eq!(fs::read(&database).unwrap(), original);
    let existing = SqliteProvider::open_existing(&database).unwrap();
    assert_business_data(&existing);
    assert!(SqliteProvider::has_current_search_text(&database).unwrap());
}
