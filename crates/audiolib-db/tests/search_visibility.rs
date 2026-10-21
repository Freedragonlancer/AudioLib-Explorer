// crates/audiolib-db/tests/search_visibility.rs
//! Legacy visibility values retain their storage while every search uses exact 0/1 predicates.
use audiolib_core::{DataProvider, FileVisibility, SearchFilters, SearchRequest, SortOrder};
use audiolib_db::{SqliteProvider, import_legacy};
use rusqlite::{Connection, params, params_from_iter, types::Value};
use std::{collections::BTreeMap, fs, path::PathBuf};

fn fixture(records: i64) -> (tempfile::TempDir, PathBuf, SqliteProvider) {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("nullable-legacy.db");
    let destination = directory.path().join("library.db");
    let conn = Connection::open(&source).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE audio_files(id INTEGER PRIMARY KEY,file_path TEXT NOT NULL UNIQUE,
         filename TEXT NOT NULL,folder_id INTEGER,sample_rate INTEGER,missing INTEGER,
         user_notes TEXT,metadata_json TEXT,private_blob BLOB);
         CREATE TABLE folders(id INTEGER PRIMARY KEY,folder_path TEXT NOT NULL UNIQUE,
         folder_name TEXT NOT NULL,parent_id INTEGER);
         INSERT INTO folders VALUES(1,'root','root',NULL),(2,'root/child','child',1);
         CREATE TABLE folder_closure(ancestor_id INTEGER,descendant_id INTEGER,depth INTEGER,
         PRIMARY KEY(ancestor_id,descendant_id));
         INSERT INTO folder_closure VALUES(1,1,0),(1,2,1),(2,2,0);",
    )
    .unwrap();
    for id in 1..=records {
        conn.execute(
            "INSERT INTO audio_files VALUES(?1,?2,?3,?4,?5,?6,?7,?8,X'0123')",
            params![
                id,
                format!("root/{id}.wav"),
                format!(
                    "{} {}.wav",
                    if id % 2 == 0 { "metal" } else { "wood" },
                    id % 7,
                ),
                if id % 3 == 0 { 1 } else { 2 },
                if id % 4 < 2 { 48_000 } else { 96_000 },
                if id % 4 == 0 { None } else { Some(0) },
                format!("retain-{id}"),
                format!(r#"{{"private":{{"id":{id}}}}}"#),
            ],
        )
        .unwrap();
    }
    drop(conn);
    let bytes = fs::read(&source).unwrap();
    import_legacy(&source, &destination).unwrap();
    assert_eq!(fs::read(&source).unwrap(), bytes);
    let provider = SqliteProvider::open_existing(&destination).unwrap();
    let migrated = Connection::open(&destination).unwrap();
    audiolib_db::register_library_functions(&migrated).unwrap();
    assert_eq!(
        migrated
            .query_row(
                "SELECT COUNT(*) FROM audio_files WHERE missing IS NULL",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        records / 4,
    );
    (directory, source, provider)
}

fn set_non_present(conn: &Connection, count: i64) {
    conn.execute(
        "UPDATE audio_files SET missing=CASE WHEN id<=?1 THEN
         CASE id%4 WHEN 0 THEN NULL WHEN 1 THEN 1 WHEN 2 THEN 2 ELSE -1 END ELSE 0 END",
        [count],
    )
    .unwrap();
}

fn oracle(conn: &Connection, request: &SearchRequest, folder_scope: bool) -> Vec<i64> {
    let mut conditions = vec![match request.filters.visibility {
        FileVisibility::Present => "af.missing=0".to_string(),
        FileVisibility::Missing => "af.missing=1".to_string(),
        FileVisibility::All => "1=1".to_string(),
    }];
    let mut values = vec![];
    let from = if request.query.is_empty() {
        "audio_files af"
    } else {
        conditions.push("audio_search MATCH ?".into());
        values.push(Value::Text(request.query.clone()));
        "audio_files af JOIN audio_search ON audio_search.rowid=af.id"
    };
    if let Some(rate) = request.filters.sample_rate {
        conditions.push("af.sample_rate=?".into());
        values.push(Value::Integer(i64::from(rate)));
    }
    if folder_scope && let Some(folder) = request.folder_id {
        conditions.push(if request.include_subfolders {
            "af.folder_id IN(SELECT descendant_id FROM folder_closure WHERE ancestor_id=?)".into()
        } else {
            "af.folder_id=?".into()
        });
        values.push(Value::Integer(folder));
    }
    let order = if request.order_by == SortOrder::Relevance && !request.query.is_empty() {
        "bm25(audio_search)"
    } else {
        "af.filename COLLATE NOCASE"
    };
    conn.prepare(&format!(
        "SELECT af.id FROM {from} WHERE {} ORDER BY {order},af.id ASC",
        conditions.join(" AND "),
    ))
    .unwrap()
    .query_map(params_from_iter(values), |row| row.get(0))
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

#[test]
fn nullable_and_nonbinary_flags_match_sql_across_sparse_dense_and_cached_paths() {
    let (_directory, _source, provider) = fixture(512);
    let conn = Connection::open(provider.database_path()).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    for unavailable in [0, 128, 129, 512] {
        set_non_present(&conn, unavailable);
        for query in ["", "metal", "wood", "absent"] {
            for visibility in [
                FileVisibility::Present,
                FileVisibility::Missing,
                FileVisibility::All,
            ] {
                for order_by in [SortOrder::Relevance, SortOrder::Filename] {
                    for (folder_id, sample_rate) in [(None, None), (Some(2), Some(48_000))] {
                        let request = SearchRequest {
                            query: query.into(),
                            filters: SearchFilters {
                                visibility,
                                sample_rate,
                                ..Default::default()
                            },
                            folder_id,
                            include_subfolders: false,
                            order_by,
                            limit: 7,
                            ..Default::default()
                        };
                        let expected = oracle(&conn, &request, true);
                        for offset in [0, 5, expected.len()] {
                            let request = SearchRequest {
                                offset,
                                ..request.clone()
                            };
                            let page = provider.search(&request).unwrap();
                            if offset == 5 && !query.is_empty() && order_by == SortOrder::Relevance
                            {
                                assert!(page.cache_hit, "{request:?}");
                            }
                            assert_eq!(page.total_count, expected.len() as u64, "{request:?}");
                            assert_eq!(
                                page.results.iter().map(|file| file.id).collect::<Vec<_>>(),
                                expected
                                    .iter()
                                    .copied()
                                    .skip(offset)
                                    .take(7)
                                    .collect::<Vec<_>>(),
                                "{unavailable} unavailable: {request:?}",
                            );
                            for file in &page.results {
                                assert_eq!(file.notes, format!("retain-{}", file.id));
                                assert_eq!(file.attributes["private"]["id"], file.id);
                                let raw: Option<i64> = conn
                                    .query_row(
                                        "SELECT missing FROM audio_files WHERE id=?1",
                                        [file.id],
                                        |row| row.get(0),
                                    )
                                    .unwrap();
                                assert_eq!(file.missing, raw != Some(0));
                            }
                        }
                        let matched = oracle(&conn, &request, false);
                        let mut expected_folders = BTreeMap::new();
                        if !matched.is_empty() {
                            expected_folders.insert(1, matched.len() as u64);
                            let child = matched.iter().filter(|id| **id % 3 != 0).count() as u64;
                            if child > 0 {
                                expected_folders.insert(2, child);
                            }
                        }
                        let folders = provider.search_folders(&request).unwrap();
                        assert_eq!(
                            folders
                                .into_iter()
                                .map(|folder| (folder.id, folder.file_count))
                                .collect::<BTreeMap<_, _>>(),
                            expected_folders,
                            "{unavailable} unavailable facets: {request:?}",
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn all_get_audio_and_history_decode_flags_without_changing_original_values() {
    let (_directory, source, provider) = fixture(5);
    let conn = Connection::open(provider.database_path()).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch("UPDATE audio_files SET missing=CASE id WHEN 1 THEN NULL WHEN 2 THEN 2 WHEN 3 THEN -1 WHEN 4 THEN 1 ELSE 0 END;").unwrap();
    let source_bytes = fs::read(&source).unwrap();
    let page = provider
        .search(&SearchRequest {
            query: "".into(),
            filters: SearchFilters {
                visibility: FileVisibility::All,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.total_count, 5);
    assert_eq!(page.results.len(), 5);
    for id in 1..=5 {
        let audio = provider.get_audio(id).unwrap();
        assert_eq!(audio.missing, id != 5);
        assert_eq!(audio.notes, format!("retain-{id}"));
        assert_eq!(audio.attributes["private"]["id"], id);
    }
    let flags = conn
        .prepare("SELECT missing FROM audio_files ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get::<_, Option<i64>>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(flags, [None, Some(2), Some(-1), Some(1), Some(0)]);
    let blobs = conn
        .prepare("SELECT private_blob FROM audio_files ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert!(blobs.iter().all(|blob| blob == &[1, 35]));
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
}

#[test]
fn non_present_probe_and_subtraction_use_the_required_partial_index() {
    let (_directory, _source, provider) = fixture(512);
    let conn = Connection::open(provider.database_path()).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    set_non_present(&conn, 128);
    for sql in [
        "SELECT id FROM audio_files INDEXED BY idx_audio_files_not_present WHERE missing IS NOT 0 LIMIT 129",
        "SELECT COUNT(*) FROM audio_files af INDEXED BY idx_audio_files_not_present WHERE af.missing IS NOT 0 AND EXISTS(SELECT 1 FROM audio_search WHERE rowid=af.id AND audio_search MATCH 'metal')",
    ] {
        let plan = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            plan.iter().any(
                |step| step.contains("USING INDEX idx_audio_files_not_present")
                    || step.contains("USING COVERING INDEX idx_audio_files_not_present")
            ),
            "{plan:?}"
        );
        assert!(
            !plan
                .iter()
                .any(|step| step == "SCAN audio_files" || step == "SCAN af"),
            "{plan:?}"
        );
    }
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE type='index' AND name='idx_audio_files_missing'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    let probed = conn.prepare("SELECT id FROM audio_files INDEXED BY idx_audio_files_not_present WHERE missing IS NOT 0 LIMIT 129").unwrap()
        .query_map([], |row| row.get::<_, i64>(0)).unwrap()
        .collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert_eq!(probed, (1..=128).collect::<Vec<_>>());
}

#[test]
fn missing_visibility_index_upgrades_owned_readonly_copy_and_same_schema_writable_open() {
    let (_directory, _source, provider) = fixture(8);
    let source = provider.database_path().to_path_buf();
    let conn = Connection::open(&source).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    set_non_present(&conn, 4);
    drop(conn);
    drop(provider);
    let conn = Connection::open(&source).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch("DROP INDEX idx_audio_files_not_present; PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;").unwrap();
    drop(conn);
    let original_bytes = fs::read(&source).unwrap();
    assert!(SqliteProvider::has_current_search_text(&source).unwrap());
    assert!(!SqliteProvider::has_current_search_state(&source).unwrap());
    let readonly = SqliteProvider::open_read_only(&source).unwrap();
    assert_ne!(readonly.database_path(), source);
    assert!(SqliteProvider::has_current_search_state(readonly.database_path()).unwrap());
    assert_eq!(
        readonly
            .search(&SearchRequest {
                query: "metal".into(),
                ..Default::default()
            })
            .unwrap()
            .total_count,
        2
    );
    assert!(readonly.get_audio(4).unwrap().missing);
    assert_eq!(fs::read(&source).unwrap(), original_bytes);
    drop(readonly);
    let writable = SqliteProvider::open_existing(&source).unwrap();
    assert_eq!(writable.database_path(), source);
    assert!(SqliteProvider::has_current_search_state(&source).unwrap());
    assert!(!SqliteProvider::upgrade_owned_snapshot(&source).unwrap());
    let conn = Connection::open(&source).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    let flags = conn
        .prepare("SELECT missing FROM audio_files WHERE id<=4 ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get::<_, Option<i64>>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(flags, [Some(1), Some(2), Some(-1), None]);
    assert_eq!(
        writable.library_info().unwrap().schema_version,
        audiolib_db::SCHEMA_VERSION
    );
}

#[cfg(feature = "scanner")]
#[test]
fn scanning_new_wav_sets_present_explicitly_on_legacy_column_without_default() {
    use audiolib_db::{ScanCancellation, ScanOptions, ScanService};
    use std::sync::Arc;
    let (directory, source, provider) = fixture(4);
    let conn = Connection::open(provider.database_path()).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute("UPDATE audio_files SET missing=NULL WHERE id=4", [])
        .unwrap();
    let provider = Arc::new(provider);
    let root = directory.path().join("sounds");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("metal new.wav");
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
    fs::write(&path, &bytes).unwrap();
    let source_bytes = fs::read(&source).unwrap();
    let progress = ScanService::new(provider.clone())
        .scan(
            &root,
            &ScanOptions::default(),
            &ScanCancellation::default(),
            |_| {},
        )
        .unwrap();
    assert_eq!(progress.added, 1);
    let audio = provider.find_audio_by_path(&path).unwrap().unwrap();
    assert!(!audio.missing);
    assert_eq!(
        conn.query_row(
            "SELECT missing FROM audio_files WHERE id=?1",
            [audio.id],
            |row| row.get::<_, Option<i64>>(0)
        )
        .unwrap(),
        Some(0)
    );
    assert_eq!(
        conn.query_row("SELECT missing FROM audio_files WHERE id=4", [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .unwrap(),
        None
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
}
