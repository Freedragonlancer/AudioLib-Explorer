// crates/audiolib-db/tests/search_folder_cache.rs
//! Repeated directory facets observe commits and cancellation through owned contracts.
use audiolib_core::{
    DataProvider, Error, FileVisibility, MetadataPatch, SearchFilters, SearchRequest, SortOrder,
};
use audiolib_db::SqliteProvider;
use rusqlite::{Connection, params};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn fixture() -> (tempfile::TempDir, PathBuf, SqliteProvider) {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("folder-cache.db");
    let provider = SqliteProvider::open(&database).unwrap();
    let conn = Connection::open(&database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO folders(id,folder_path,folder_name,parent_id) VALUES
         (1,'root','root',NULL),(2,'root/left','left',1),(3,'root/right','right',1);
         INSERT INTO folder_closure VALUES
         (1,1,0),(2,2,0),(3,3,0),(1,2,1),(1,3,1);
         INSERT INTO audio_files(id,file_path,filename,folder_id,missing,user_tags,user_rating,favorite,sample_rate) VALUES(1,'root/left/plain-a.wav','plain-a.wav',2,0,'metal',5,1,48000),
         (2,'root/left/plain-b.wav','plain-b.wav',2,0,'wood',1,0,96000),
         (3,'root/right/plain-c.wav','plain-c.wav',3,1,'metal',4,1,48000),
         (4,'root/right/plain-d.wav','plain-d.wav',3,0,'wood',4,0,96000);",
    )
    .unwrap();
    drop(conn);
    (directory, database, provider)
}

fn request(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.into(),
        ..Default::default()
    }
}

fn counts(provider: &SqliteProvider, request: &SearchRequest) -> BTreeMap<i64, u64> {
    provider
        .search_folders(request)
        .unwrap()
        .into_iter()
        .map(|folder| (folder.id, folder.file_count))
        .collect()
}

fn oracle(database: &Path, request: &SearchRequest) -> BTreeMap<i64, u64> {
    // These fixed terms are intentionally independent of the new cache/query compiler.
    assert!(["metal", "wood"].contains(&request.query.as_str()));
    let visibility = match request.filters.visibility {
        FileVisibility::Present => "af.missing=0",
        FileVisibility::Missing => "af.missing=1",
        FileVisibility::All => "1=1",
    };
    let conn = Connection::open(database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    let sql = format!(
        "SELECT closure.ancestor_id,COUNT(*) FROM audio_files af
         JOIN audio_search ON audio_search.rowid=af.id
         JOIN folder_closure closure ON closure.descendant_id=af.folder_id
         WHERE audio_search MATCH ?1 AND {visibility}
         AND (?2=0 OR COALESCE(af.favorite,0)=1)
         AND (?3 IS NULL OR af.sample_rate=?3)
         AND (?4 IS NULL OR af.duration>=?4)
         AND (?5 IS NULL OR af.duration<=?5)
         GROUP BY closure.ancestor_id"
    );
    conn.prepare(&sql)
        .unwrap()
        .query_map(
            params![
                request.query,
                request.filters.favorites_only,
                request.filters.sample_rate.map(i64::from),
                request.filters.duration_min,
                request.filters.duration_max,
            ],
            |row| Ok((row.get(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap()
        .map(|row| {
            let (id, count) = row.unwrap();
            (id, u64::try_from(count).unwrap())
        })
        .collect()
}

#[test]
fn own_and_external_commits_refresh_directory_counts_and_owned_folder_values() {
    let (_directory, database, provider) = fixture();
    let query = request("metal");
    assert!(!provider.search(&query).unwrap().cache_hit);
    assert_eq!(counts(&provider, &query), BTreeMap::from([(1, 1), (2, 1)]));
    let all = SearchRequest {
        filters: SearchFilters {
            visibility: FileVisibility::All,
            ..Default::default()
        },
        ..query.clone()
    };
    let previous = provider.search_folders(&all).unwrap();
    provider
        .update_metadata(
            2,
            &MetadataPatch {
                tags: Some("metal".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(counts(&provider, &query), BTreeMap::from([(1, 2), (2, 2)]));
    assert!(!provider.search(&query).unwrap().cache_hit);
    provider
        .update_metadata(
            1,
            &MetadataPatch {
                tags: Some("wood".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(counts(&provider, &query), oracle(&database, &query));

    assert!(!provider.search(&all).unwrap().cache_hit);

    let conn = Connection::open(&database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "BEGIN;
         UPDATE audio_files SET folder_id=3 WHERE id=2;
         UPDATE audio_files SET missing=0 WHERE id=3;
         UPDATE audio_files SET user_tags='metal' WHERE id=4;
         UPDATE folders SET folder_name='renamed',folder_path='root/renamed' WHERE id=3;
         COMMIT;",
    )
    .unwrap();
    drop(conn);
    assert_eq!(counts(&provider, &query), BTreeMap::from([(1, 3), (3, 3)]));
    assert_eq!(counts(&provider, &query), oracle(&database, &query));
    let current = provider.search_folders(&query).unwrap();
    let renamed = current.iter().find(|folder| folder.id == 3).unwrap();
    assert_eq!(renamed.name, "renamed");
    assert_eq!(renamed.path, "root/renamed");
    let old = previous.iter().find(|folder| folder.id == 3).unwrap();
    assert_eq!(old.name, "right");
    assert_eq!(old.file_count, 1);
    assert!(!provider.search(&all).unwrap().cache_hit);
    provider
        .update_metadata(
            3,
            &MetadataPatch {
                tags: Some("wood".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(counts(&provider, &query), BTreeMap::from([(1, 2), (3, 2)]));
    assert_eq!(counts(&provider, &query), oracle(&database, &query));

    assert!(!provider.search(&all).unwrap().cache_hit);
    let conn = Connection::open(&database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute("DELETE FROM audio_files WHERE id=2", [])
        .unwrap();
    drop(conn);
    assert_eq!(counts(&provider, &query), oracle(&database, &query));
    assert_eq!(counts(&provider, &query), BTreeMap::from([(1, 1), (3, 1)]));
}

#[test]
fn repeated_cross_query_facets_match_the_sql_oracle_for_filters_and_visibility() {
    let (_directory, database, provider) = fixture();
    for query in ["metal", "wood"] {
        assert!(!provider.search(&request(query)).unwrap().cache_hit);
    }
    let queries = [
        request("metal"),
        request("wood"),
        SearchRequest {
            filters: SearchFilters {
                visibility: FileVisibility::All,
                ..Default::default()
            },
            ..request("metal")
        },
        SearchRequest {
            filters: SearchFilters {
                visibility: FileVisibility::Missing,
                ..Default::default()
            },
            ..request("metal")
        },
        SearchRequest {
            filters: SearchFilters {
                favorites_only: true,
                ..Default::default()
            },
            ..request("wood")
        },
        SearchRequest {
            filters: SearchFilters {
                sample_rate: Some(48_000),
                ..Default::default()
            },
            ..request("metal")
        },
        SearchRequest {
            folder_id: Some(2),
            include_subfolders: false,
            offset: 5000,
            limit: 1,
            order_by: SortOrder::Filename,
            descending: true,
            ..request("metal")
        },
    ];
    for _ in 0..3 {
        for query in &queries {
            assert_eq!(counts(&provider, query), oracle(&database, query));
        }
    }
}

#[test]
fn precancelled_uncached_and_cached_facets_do_not_poison_later_reads() {
    let (_directory, database, provider) = fixture();
    let query = request("metal");
    let cancel = Arc::new(AtomicBool::new(true));
    assert!(matches!(
        provider.search_folders_cancellable(&query, cancel.clone()),
        Err(Error::Cancelled)
    ));
    cancel.store(false, Ordering::Release);
    assert_eq!(counts(&provider, &query), oracle(&database, &query));
    assert!(!provider.search(&query).unwrap().cache_hit);
    assert!(provider.search(&query).unwrap().cache_hit);
    cancel.store(true, Ordering::Release);
    assert!(matches!(
        provider.search_folders_cancellable(&query, cancel.clone()),
        Err(Error::Cancelled)
    ));
    cancel.store(false, Ordering::Release);
    let actual: BTreeMap<_, _> = provider
        .search_folders_cancellable(&request("wood"), cancel)
        .unwrap()
        .into_iter()
        .map(|folder| (folder.id, folder.file_count))
        .collect();
    assert_eq!(actual, oracle(&database, &request("wood")));
    assert_eq!(counts(&provider, &query), oracle(&database, &query));
    assert!(provider.search(&query).unwrap().cache_hit);
}

#[test]
fn chunked_ranked_ids_preserve_visibility_scope_and_signed_identities() {
    let (_directory, database, provider) = fixture();
    let conn = Connection::open(&database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "WITH RECURSIVE records(id) AS (
           VALUES(10) UNION ALL SELECT id+1 FROM records WHERE id<8210
         ) INSERT INTO audio_files(id,file_path,filename,folder_id,missing)
           SELECT id,'generated/'||id||'.wav','metal-'||id||'.wav',
             CASE id%3 WHEN 0 THEN NULL WHEN 1 THEN 2 ELSE 3 END,
             CASE id%5 WHEN 0 THEN -1 WHEN 1 THEN 1 WHEN 2 THEN 2 ELSE 0 END
           FROM records;",
    )
    .unwrap();
    for (id, missing) in [(i64::MIN, 0), (i64::MAX, 1)] {
        conn.execute(
            "INSERT INTO audio_files(id,file_path,filename,folder_id,missing)
             VALUES(?1,?2,'metal-boundary.wav',2,?3)",
            params![id, format!("boundary/{id}.wav"), missing],
        )
        .unwrap();
    }
    drop(conn);

    // A page with no returned rows still retains the complete query ranking,
    // including records outside its requested folder and visibility selection.
    let scoped = SearchRequest {
        folder_id: Some(2),
        include_subfolders: false,
        offset: 20_000,
        limit: 1,
        ..request("metal")
    };
    let page = provider.search(&scoped).unwrap();
    assert!(page.results.is_empty());
    assert!(!page.cache_hit);
    assert!(provider.search(&scoped).unwrap().cache_hit);

    for visibility in [
        FileVisibility::Present,
        FileVisibility::Missing,
        FileVisibility::All,
    ] {
        for order_by in [SortOrder::Relevance, SortOrder::Filename, SortOrder::Tags] {
            let facets = SearchRequest {
                folder_id: Some(3),
                include_subfolders: false,
                offset: 12_345,
                limit: 1,
                order_by,
                descending: true,
                filters: SearchFilters {
                    visibility,
                    ..Default::default()
                },
                ..request("metal")
            };
            assert_eq!(counts(&provider, &facets), oracle(&database, &facets));
        }
    }
}

fn filtered_fixture(extra_candidates: bool) -> (tempfile::TempDir, PathBuf, SqliteProvider) {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("small-filtered-ranking.db");
    let legacy = directory.path().join("legacy-nullable.db");
    let conn = Connection::open(&legacy).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    // Migration retains a nullable legacy visibility column. This database is
    // created only inside the owned fixture, never from a working library.
    conn.execute_batch(
        "CREATE TABLE audio_files(id INTEGER PRIMARY KEY,file_path TEXT NOT NULL UNIQUE,
           filename TEXT NOT NULL,folder_id INTEGER,missing INTEGER,sample_rate INTEGER,
           duration REAL);",
    )
    .unwrap();
    drop(conn);
    audiolib_db::import_legacy(&legacy, &database).unwrap();
    let provider = SqliteProvider::open(&database).unwrap();
    let mut conn = Connection::open(&database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO folders(id,folder_path,folder_name,parent_id) VALUES
         (1,'root','root',NULL),(2,'root/left','left',1),(3,'root/right','right',1);
         INSERT INTO folder_closure VALUES
         (1,1,0),(2,2,0),(3,3,0),(1,2,1),(1,3,1);",
    )
    .unwrap();
    let tx = conn.transaction().unwrap();
    for (id, folder, missing, rate, duration) in [
        (1, Some(2), Some(0), 48_000, 1.0),
        (2, Some(3), Some(0), 48_000, 2.0),
        (3, Some(2), Some(1), 48_000, 1.0),
        (4, Some(3), None, 48_000, 1.0),
        (5, Some(3), Some(2), 48_000, 1.0),
        (6, Some(3), Some(0), 96_000, 1.0),
        (7, Some(2), Some(0), 48_000, 10.0),
        (8, None, Some(0), 48_000, 1.0),
    ] {
        tx.execute(
            "INSERT INTO audio_files(id,file_path,filename,folder_id,missing,sample_rate,
             duration,user_notes,metadata_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                id,
                format!("root/{id}.wav"),
                format!("metal {id}.wav"),
                folder,
                missing,
                rate,
                duration,
                format!("retained-{id}"),
                format!(r#"{{"fixture_id":{id}}}"#),
            ],
        )
        .unwrap();
    }
    tx.execute(
        "INSERT INTO audio_files(id,file_path,filename,folder_id,missing,sample_rate,duration)
         VALUES(9,'root/wood.wav','wood.wav',2,0,48000,1.0)",
        [],
    )
    .unwrap();
    if extra_candidates {
        // The full metal ranking has eight IDs, so 8301 unrelated eligible
        // candidates exceed the production 8*R+8192 budget. Facets must discard
        // that incomplete intersection and use their single-MATCH fallback.
        tx.execute_batch(
            "WITH RECURSIVE ids(id) AS (
               VALUES(100) UNION ALL SELECT id+1 FROM ids WHERE id<8400
             ) INSERT INTO audio_files(id,file_path,filename,folder_id,missing,sample_rate,duration)
               SELECT id,'root/nonmatch-'||id||'.wav','wood.wav',2,0,48000,1.0 FROM ids;",
        )
        .unwrap();
    }
    tx.commit().unwrap();
    drop(conn);
    (directory, database, provider)
}

#[test]
fn small_filtered_rankings_and_candidate_overflow_preserve_full_facets_and_scoped_pages() {
    for extra_candidates in [false, true] {
        let (_directory, database, provider) = filtered_fixture(extra_candidates);
        let requests = [
            FileVisibility::Present,
            FileVisibility::Missing,
            FileVisibility::All,
        ]
        .map(|visibility| SearchRequest {
            filters: SearchFilters {
                visibility,
                sample_rate: Some(48_000),
                duration_min: Some(0.0),
                duration_max: Some(5.0),
                ..Default::default()
            },
            folder_id: Some(2),
            include_subfolders: false,
            offset: 1,
            limit: 1,
            ..request("metal")
        });
        // No page search has populated this provider's ranking cache yet.
        for request in &requests {
            assert_eq!(counts(&provider, request), oracle(&database, request));
        }
        let all = SearchRequest {
            filters: SearchFilters {
                visibility: FileVisibility::All,
                ..Default::default()
            },
            ..request("metal")
        };
        assert_eq!(provider.search(&all).unwrap().total_count, 8);
        let conn = Connection::open(&database).unwrap();
        audiolib_db::register_library_functions(&conn).unwrap();
        for request in &requests {
            let expected_folders = match request.filters.visibility {
                FileVisibility::Present => BTreeMap::from([(1, 2), (2, 1), (3, 1)]),
                FileVisibility::Missing => BTreeMap::from([(1, 1), (2, 1)]),
                FileVisibility::All => BTreeMap::from([(1, 5), (2, 2), (3, 3)]),
            };
            assert_eq!(oracle(&database, request), expected_folders);
            assert_eq!(counts(&provider, request), expected_folders);
            let visibility = match request.filters.visibility {
                FileVisibility::Present => "af.missing=0",
                FileVisibility::Missing => "af.missing=1",
                FileVisibility::All => "1=1",
            };
            let expected_ids = conn
                .prepare(&format!(
                    "SELECT af.id FROM audio_search CROSS JOIN audio_files af
                     ON af.id=audio_search.rowid WHERE audio_search MATCH 'metal'
                     AND {visibility} AND af.sample_rate=48000
                     AND af.duration>=0.0 AND af.duration<=5.0 AND af.folder_id=2
                     ORDER BY bm25(audio_search),af.id ASC"
                ))
                .unwrap()
                .query_map([], |row| row.get::<_, i64>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let page = provider.search(request).unwrap();
            assert!(page.cache_hit);
            assert_eq!(page.total_count, expected_ids.len() as u64);
            assert_eq!(
                page.results.iter().map(|file| file.id).collect::<Vec<_>>(),
                expected_ids
                    .into_iter()
                    .skip(request.offset)
                    .take(request.limit)
                    .collect::<Vec<_>>()
            );
            for file in &page.results {
                assert_eq!(file.notes, format!("retained-{}", file.id));
                assert_eq!(file.attributes["fixture_id"], file.id);
            }
        }
        assert_eq!(
            conn.query_row("SELECT missing FROM audio_files WHERE id=4", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .unwrap(),
            None
        );
    }
}
