// crates/audiolib-db/tests/feature_revision.rs
//! Additive implementation revisions never downgrade a future library or snapshot.
use audiolib_core::{DataProvider, Error, MaintenanceAction, SearchRequest};
use audiolib_db::{SnapshotUpgradeProgress, SqliteProvider};
use rusqlite::Connection;
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

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
    for feature in [
        "stable_audio_ids",
        "foreign_key_indexes",
        "tokenization",
        "search_indexes",
    ] {
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
                "UPDATE audiolib_schema_features SET version=?1 WHERE feature=?2",
                rusqlite::params![if feature == "tokenization" { 4 } else { 2 }, feature],
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

#[test]
fn missing_performance_indexes_upgrade_only_owned_snapshot_and_keep_search_text() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("missing-search-index.db");
    fixture(&database);
    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "DROP INDEX idx_audio_search_facets;
         DROP INDEX idx_audio_visible_tagged_folders;
         DELETE FROM audiolib_schema_features WHERE feature='search_indexes';",
    )
    .unwrap();
    let before_text: String = conn
        .query_row(
            "SELECT filename_tokenized FROM audio_files WHERE id=42",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(conn);
    let original = fs::read(&database).unwrap();
    assert!(SqliteProvider::has_current_search_text(&database).unwrap());
    assert!(!SqliteProvider::has_current_search_state(&database).unwrap());
    let readonly = SqliteProvider::open_read_only(&database).unwrap();
    assert_ne!(readonly.database_path(), database.as_path());
    assert_business_data(&readonly);
    let upgraded = Connection::open(readonly.database_path()).unwrap();
    let after_text: String = upgraded
        .query_row(
            "SELECT filename_tokenized FROM audio_files WHERE id=42",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        before_text, after_text,
        "index-only upgrade must not retokenize text"
    );
    assert!(SqliteProvider::has_current_search_state(readonly.database_path()).unwrap());
    drop(upgraded);
    drop(readonly);
    assert_eq!(fs::read(&database).unwrap(), original);
    let existing = SqliteProvider::open_existing(&database).unwrap();
    assert_business_data(&existing);
    assert!(SqliteProvider::has_current_search_state(&database).unwrap());
}

fn revision_two_fold_fixture(path: &Path) {
    let provider = SqliteProvider::open(path).unwrap();
    let conn = Connection::open(path).unwrap();
    conn.execute(
        "INSERT INTO audio_files(id,file_path,filename,user_tags,user_notes,favorite,metadata_json)
         VALUES(42,'N:/fixture/plain.wav','plain.wav',?1,'anchor',1,?2)",
        rusqlite::params![
            "ß",
            r#"{"private":"keep","wav_metadata":{"fields":{"keywords":"ﬃ"}}}"#,
        ],
    )
    .unwrap();
    drop(conn);
    provider
        .maintain_library(MaintenanceAction::RebuildSearch)
        .unwrap();
    provider.record_play(42).unwrap();
    drop(provider);
    let conn = Connection::open(path).unwrap();
    // Ordinary words keep exactly the revision-2 representation. Its only
    // differing values here are the original one-grapheme fold tokens below;
    // the existing triggers synchronize these old values with valid postings.
    conn.execute_batch(
        "UPDATE audio_files SET user_tags_tokenized='s000073000073',
           bwf_description_tokenized='s000066000066000069' WHERE id=42;
         UPDATE audiolib_schema_features SET version=2 WHERE feature='tokenization';",
    )
    .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM audio_search WHERE audio_search MATCH
             'user_tags_tokenized : \"g000073000073\" OR bwf_description_tokenized : \"g000066000069\"'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
    let mode: String = conn
        .query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
}

fn assert_fold_upgrade(provider: &SqliteProvider) {
    for query in ["ss", "fi", "ffi", "ß anchor", "ﬃ anchor"] {
        let page = provider
            .search(&SearchRequest {
                query: query.into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.total_count, 1, "{query}");
        assert_eq!(page.results[0].id, 42, "{query}");
    }
    let file = provider.get_audio(42).unwrap();
    assert_eq!(file.tags, "ß");
    assert_eq!(file.notes, "anchor");
    assert!(file.favorite);
    assert_eq!(file.play_count, 0);
    assert_eq!(file.attributes["private"], "keep");
    assert_eq!(file.attributes["wav_metadata"]["fields"]["keywords"], "ﬃ");
    assert_eq!(provider.play_history(10, 0).unwrap().len(), 1);
    let conn = Connection::open(provider.database_path()).unwrap();
    let revision: i64 = conn
        .query_row(
            "SELECT version FROM audiolib_schema_features WHERE feature='tokenization'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(revision, 3);
}

#[test]
fn revision_two_fold_tokens_upgrade_readonly_copies_owned_and_existing_libraries_once() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("fold-v2.db");
    revision_two_fold_fixture(&source);
    let original = fs::read(&source).unwrap();
    assert!(!SqliteProvider::has_current_search_state(&source).unwrap());
    let readonly = SqliteProvider::open_read_only(&source).unwrap();
    assert_ne!(readonly.database_path(), source.as_path());
    assert_fold_upgrade(&readonly);
    drop(readonly);
    assert_eq!(fs::read(&source).unwrap(), original);

    let owned = directory.path().join("owned-fold.db");
    fs::copy(&source, &owned).unwrap();
    assert!(SqliteProvider::upgrade_owned_snapshot(&owned).unwrap());
    let provider = SqliteProvider::open_read_only(&owned).unwrap();
    assert_eq!(provider.database_path(), owned.as_path());
    assert_fold_upgrade(&provider);
    drop(provider);
    let upgraded = fs::read(&owned).unwrap();
    assert!(!SqliteProvider::upgrade_owned_snapshot(&owned).unwrap());
    assert_eq!(fs::read(&owned).unwrap(), upgraded);

    let existing = directory.path().join("existing-fold.db");
    fs::copy(&source, &existing).unwrap();
    let provider = SqliteProvider::open_existing(&existing).unwrap();
    assert_fold_upgrade(&provider);
    drop(provider);
    let reopened = SqliteProvider::open_existing(&existing).unwrap();
    assert_fold_upgrade(&reopened);
    assert_eq!(fs::read(&source).unwrap(), original);
}

#[test]
fn cancelled_revision_two_fold_upgrade_rolls_back_text_and_revision() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("fold-v2.db");
    revision_two_fold_fixture(&source);
    let original = fs::read(&source).unwrap();
    let owned = directory.path().join("cancelled-fold.db");
    fs::copy(&source, &owned).unwrap();
    let cancel = AtomicBool::new(false);
    let mut retokenized = false;
    let result = SqliteProvider::upgrade_owned_snapshot_with_progress_cancellable(
        &owned,
        &cancel,
        |event| {
            if matches!(
                event,
                SnapshotUpgradeProgress::Tokenizing {
                    completed: 1,
                    total: 1
                }
            ) {
                retokenized = true;
                cancel.store(true, Ordering::Release);
            }
        },
    );
    assert!(retokenized);
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert_eq!(fs::read(&owned).unwrap(), original);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert!(!SqliteProvider::has_current_search_text(&owned).unwrap());
    assert!(SqliteProvider::check_owned_file(&owned).unwrap().is_ok());
    let conn = Connection::open(&owned).unwrap();
    let revision: i64 = conn
        .query_row(
            "SELECT version FROM audiolib_schema_features WHERE feature='tokenization'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(revision, 2);
}
