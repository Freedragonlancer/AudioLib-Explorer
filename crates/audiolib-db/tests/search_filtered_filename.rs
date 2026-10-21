// crates/audiolib-db/tests/search_filtered_filename.rs
//! Filtered filename pages retain native SQLite order, complete counts and annotations.
use audiolib_core::{
    DataProvider, Error, MaintenanceAction, SearchFilters, SearchRequest, SortOrder,
};
use audiolib_db::SqliteProvider;
use rusqlite::{Connection, params, params_from_iter, types::Value};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

fn fixture() -> (tempfile::TempDir, PathBuf, SqliteProvider) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("filtered-filename.db");
    let legacy = directory.path().join("legacy-nullable.db");
    let conn = Connection::open(&legacy).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE audio_files(id INTEGER PRIMARY KEY,file_path TEXT NOT NULL UNIQUE,
         filename TEXT NOT NULL,missing INTEGER);",
    )
    .unwrap();
    drop(conn);
    audiolib_db::import_legacy(&legacy, &path).unwrap();
    let provider = SqliteProvider::open(&path).unwrap();
    let mut conn = Connection::open(&path).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    for (id, name, rate, duration, missing) in [
        (i64::MIN, "A", Some(48_000), Some(0.0), Some(0)),
        (-9, "a", Some(48_000), Some(1.0), Some(0)),
        (0, "Z", Some(48_000), None, Some(0)),
        (1, "Ä", Some(48_000), Some(2.0), Some(0)),
        (7, "ä", Some(48_000), Some(2.0), Some(0)),
        (9, "b", Some(48_000), Some(9.0), Some(0)),
        (11, "a", Some(96_000), Some(1.0), Some(0)),
        (17, "m", Some(48_000), Some(1.0), Some(1)),
        (18, "n", Some(48_000), Some(1.0), None),
        (19, "q", Some(48_000), Some(1.0), Some(2)),
        (23, "c", None, Some(1.0), Some(0)),
        (i64::MAX, "B", Some(48_000), Some(1.0), Some(0)),
    ] {
        tx.execute(
            "INSERT INTO audio_files(id,file_path,filename,sample_rate,duration,missing,
             favorite,user_notes,metadata_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                id,
                format!("fixture/{id}.wav"),
                format!("explosion {name}.wav"),
                rate,
                duration,
                missing,
                i64::from(id % 2 == 0),
                format!("retained-{id}"),
                serde_json::json!({"original_id": id}).to_string(),
            ],
        )
        .unwrap();
    }
    tx.execute(
        "INSERT INTO audio_files(id,file_path,filename,sample_rate,duration,missing)
         VALUES(100,'fixture/wood.wav','wood.wav',48000,1.0,0)",
        [],
    )
    .unwrap();
    tx.commit().unwrap();
    drop(conn);
    provider
        .maintain_library(MaintenanceAction::RebuildSearch)
        .unwrap();
    (directory, path, provider)
}

fn request() -> SearchRequest {
    SearchRequest {
        query: "plos".into(),
        order_by: SortOrder::Filename,
        filters: SearchFilters {
            sample_rate: Some(48_000),
            ..Default::default()
        },
        limit: 3,
        ..Default::default()
    }
}

fn oracle(conn: &Connection, request: &SearchRequest) -> (u64, Vec<i64>) {
    // Every fragment match in this fixture contains the original complete token.
    // Native MATCH + JOIN bypasses both candidate collection and filename indexing.
    let from = "FROM audio_search CROSS JOIN audio_files af ON af.id=audio_search.rowid
        WHERE audio_search MATCH 'explosion' AND af.missing=0
        AND (?1 IS NULL OR af.sample_rate=?1)
        AND (?2 IS NULL OR af.duration>=?2) AND (?3 IS NULL OR af.duration<=?3)
        AND (?4=0 OR af.favorite=1)";
    let mut values = vec![
        request
            .filters
            .sample_rate
            .map_or(Value::Null, |value| Value::Integer(i64::from(value))),
        request
            .filters
            .duration_min
            .map_or(Value::Null, Value::Real),
        request
            .filters
            .duration_max
            .map_or(Value::Null, Value::Real),
        Value::Integer(i64::from(request.filters.favorites_only)),
    ];
    let total = conn
        .query_row(
            &format!("SELECT COUNT(*) {from}"),
            params_from_iter(&values),
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let total = u64::try_from(total).unwrap();
    let ids = if request.offset as u64 >= total {
        Vec::new()
    } else {
        let direction = if request.descending { "DESC" } else { "ASC" };
        values.push(Value::Integer(request.limit as i64));
        values.push(Value::Integer(i64::try_from(request.offset).unwrap()));
        conn.prepare(&format!(
            "SELECT af.id {from} ORDER BY af.filename COLLATE NOCASE {direction},af.id ASC
             LIMIT ?5 OFFSET ?6"
        ))
        .unwrap()
        .query_map(params_from_iter(&values), |row| row.get::<_, i64>(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    };
    (total, ids)
}

fn assert_page(provider: &SqliteProvider, conn: &Connection, request: &SearchRequest) {
    let (expected_total, expected_ids) = oracle(conn, request);
    let page = provider.search(request).unwrap();
    assert_eq!(page.total_count, expected_total);
    assert_eq!(page.returned_count, expected_ids.len());
    assert_eq!(
        page.results.iter().map(|file| file.id).collect::<Vec<_>>(),
        expected_ids
    );
    assert!(!page.cache_hit);
    assert!(page.execution_time_ms.is_finite() && page.execution_time_ms >= 0.0);
    for file in page.results {
        assert_eq!(file.notes, format!("retained-{}", file.id));
        assert_eq!(file.attributes["original_id"], file.id);
    }
}

#[test]
fn complete_filtered_counts_and_both_filename_directions_match_native_nocase() {
    let (_directory, path, provider) = fixture();
    let conn = Connection::open(path).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    let filters = [
        SearchFilters {
            sample_rate: Some(48_000),
            ..Default::default()
        },
        SearchFilters {
            duration_max: Some(5.0),
            ..Default::default()
        },
        SearchFilters {
            sample_rate: Some(48_000),
            duration_min: Some(0.0),
            duration_max: Some(5.0),
            ..Default::default()
        },
        SearchFilters {
            sample_rate: Some(44_100),
            ..Default::default()
        },
        SearchFilters {
            sample_rate: Some(48_000),
            duration_max: Some(0.0),
            ..Default::default()
        },
    ];
    for filters in filters {
        for descending in [false, true] {
            for offset in [0, 1, 5, 100, usize::MAX] {
                assert_page(
                    &provider,
                    &conn,
                    &SearchRequest {
                        filters: filters.clone(),
                        descending,
                        offset,
                        ..request()
                    },
                );
            }
        }
    }
    // The optimized filename path must not publish unscored IDs as a ranking.
    let relevance = SearchRequest {
        order_by: SortOrder::Relevance,
        ..request()
    };
    assert!(!provider.search(&relevance).unwrap().cache_hit);
    assert!(provider.search(&relevance).unwrap().cache_hit);
    assert_eq!(
        conn.query_row("SELECT missing FROM audio_files WHERE id=18", [], |row| row
            .get::<_, Option<i64>>(0))
            .unwrap(),
        None
    );
}

#[test]
fn old_or_incompatible_optional_indexes_and_extra_filters_keep_complete_fallback() {
    let (_directory, path, provider) = fixture();
    let conn = Connection::open(path).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    assert_page(
        &provider,
        &conn,
        &SearchRequest {
            filters: SearchFilters {
                sample_rate: Some(48_000),
                favorites_only: true,
                ..Default::default()
            },
            ..request()
        },
    );
    for replacement in [
        "DROP INDEX idx_audio_visible_name; CREATE INDEX idx_audio_visible_name
         ON audio_files(filename COLLATE BINARY,id) WHERE missing=0;",
        "DROP INDEX idx_audio_visible_name; CREATE INDEX idx_audio_visible_name
         ON audio_files(filename COLLATE NOCASE,id) WHERE missing=1;",
        "DROP INDEX idx_audio_visible_name;",
        "CREATE INDEX idx_audio_visible_name ON audio_files(filename COLLATE NOCASE,id)
         WHERE missing=0; DROP INDEX idx_audio_visible_filter; CREATE INDEX idx_audio_visible_filter
         ON audio_files(sample_rate,duration,id) WHERE missing=1;",
        "DROP INDEX idx_audio_visible_filter;",
    ] {
        conn.execute_batch(replacement).unwrap();
        for descending in [false, true] {
            assert_page(
                &provider,
                &conn,
                &SearchRequest {
                    descending,
                    ..request()
                },
            );
        }
    }
    assert_page(
        &provider,
        &conn,
        &SearchRequest {
            filters: SearchFilters {
                sample_rate: Some(48_000),
                favorites_only: true,
                ..Default::default()
            },
            ..request()
        },
    );
}

#[test]
fn unfiltered_filename_pages_require_a_current_exact_all_present_proof() {
    let (_directory, path, provider) = fixture();
    let conn = Connection::open(path).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    let plain = SearchRequest {
        filters: SearchFilters::default(),
        ..request()
    };
    // The nullable/nonbinary fixture first exercises the original full fallback.
    assert_page(&provider, &conn, &plain);
    conn.execute("UPDATE audio_files SET missing=0", [])
        .unwrap();
    for descending in [false, true] {
        for offset in [0, 3, 10, usize::MAX] {
            assert_page(
                &provider,
                &conn,
                &SearchRequest {
                    descending,
                    offset,
                    ..plain.clone()
                },
            );
        }
    }
    // A subsequent commit invalidates the proof, including exact-NULL/nonbinary
    // cases which must never be mistaken for present rows by COALESCE/bool casts.
    for missing in [None, Some(1), Some(2)] {
        conn.execute(
            "UPDATE audio_files SET missing=?1 WHERE id=?2",
            params![missing, i64::MIN],
        )
        .unwrap();
        assert_page(&provider, &conn, &plain);
        conn.execute("UPDATE audio_files SET missing=0 WHERE id=?1", [i64::MIN])
            .unwrap();
    }
}

#[test]
fn precancelled_filtered_filename_search_preserves_following_reader_use() {
    let (_directory, path, provider) = fixture();
    let conn = Connection::open(path).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    assert!(matches!(
        provider.search_cancellable(&request(), Arc::new(AtomicBool::new(true))),
        Err(Error::Cancelled)
    ));
    assert_page(&provider, &conn, &request());
}
