// crates/audiolib-db/tests/related_indexes.rs
//! Record removal must not rescan the entire annotation/cache/history population.
use audiolib_db::SqliteProvider;
use rusqlite::Connection;

fn assert_indexed_child_deletes(conn: &Connection) {
    for table in [
        "playlist_items",
        "project_references",
        "waveform_cache",
        "play_history",
    ] {
        let plan: Vec<String> = conn
            .prepare(&format!(
                "EXPLAIN QUERY PLAN DELETE FROM {table} WHERE audio_file_id=?1"
            ))
            .unwrap()
            .query_map([12345], |row| row.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|step| step.contains("SEARCH") && step.contains(table)),
            "{table}: {plan:?}"
        );
        assert!(
            !plan
                .iter()
                .any(|step| step.contains(&format!("SCAN {table}"))),
            "{table}: {plan:?}"
        );
    }
    let plan: Vec<String> = conn
        .prepare("EXPLAIN QUERY PLAN DELETE FROM audio_files WHERE id=?1")
        .unwrap()
        .query_map([12345], |row| row.get(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for table in [
        "playlist_items",
        "project_references",
        "waveform_cache",
        "play_history",
    ] {
        assert!(
            !plan
                .iter()
                .any(|step| step.contains(&format!("SCAN {table}"))),
            "foreign-key check {table}: {plan:?}"
        );
    }
}

#[test]
fn new_libraries_use_indexed_related_record_and_foreign_key_lookups() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.db");
    let provider = SqliteProvider::open(&path).unwrap();
    let conn = Connection::open(provider.database_path()).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    assert_indexed_child_deletes(&conn);
}

#[test]
fn existing_schema_three_libraries_receive_related_indexes_without_table_rebuild() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.db");
    let provider = SqliteProvider::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    let rootpage: i64 = conn
        .query_row(
            "SELECT rootpage FROM sqlite_schema WHERE name='audio_files'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute_batch(
        "DROP INDEX idx_playlist_items_audio_file; DROP INDEX idx_project_references_audio_file;
         DROP INDEX idx_waveform_cache_audio_file; DROP INDEX idx_play_history_audio_file;
         DELETE FROM audiolib_schema_features WHERE feature='foreign_key_indexes';",
    )
    .unwrap();
    drop(conn);
    drop(provider);
    let provider = SqliteProvider::open_existing(&path).unwrap();
    let conn = Connection::open(provider.database_path()).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT rootpage FROM sqlite_schema WHERE name='audio_files'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        rootpage
    );
    assert_indexed_child_deletes(&conn);
}
