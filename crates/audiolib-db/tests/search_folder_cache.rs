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
    conn.execute_batch(
        "INSERT INTO folders(id,folder_path,folder_name,parent_id) VALUES
         (1,'root','root',NULL),(2,'root/left','left',1),(3,'root/right','right',1);
         INSERT INTO folder_closure VALUES
         (1,1,0),(2,2,0),(3,3,0),(1,2,1),(1,3,1);
         INSERT INTO audio_files(id,file_path,filename,folder_id,missing,user_tags,
           user_tags_tokenized,user_rating,favorite,sample_rate) VALUES
         (1,'root/left/plain-a.wav','plain-a.wav',2,0,'metal','metal',5,1,48000),
         (2,'root/left/plain-b.wav','plain-b.wav',2,0,'wood','wood',1,0,96000),
         (3,'root/right/plain-c.wav','plain-c.wav',3,1,'metal','metal',4,1,48000),
         (4,'root/right/plain-d.wav','plain-d.wav',3,0,'wood','wood',4,0,96000);",
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
        FileVisibility::Present => "COALESCE(af.missing,0)=0",
        FileVisibility::Missing => "af.missing=1",
        FileVisibility::All => "1=1",
    };
    let conn = Connection::open(database).unwrap();
    let sql = format!(
        "SELECT closure.ancestor_id,COUNT(*) FROM audio_files af
         JOIN audio_search ON audio_search.rowid=af.id
         JOIN folder_closure closure ON closure.descendant_id=af.folder_id
         WHERE audio_search MATCH ?1 AND {visibility}
         AND (?2=0 OR COALESCE(af.favorite,0)=1)
         AND (?3 IS NULL OR af.sample_rate=?3)
         GROUP BY closure.ancestor_id"
    );
    conn.prepare(&sql)
        .unwrap()
        .query_map(
            params![
                request.query,
                request.filters.favorites_only,
                request.filters.sample_rate.map(i64::from)
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

    let conn = Connection::open(&database).unwrap();
    conn.execute_batch(
        "BEGIN;
         UPDATE audio_files SET folder_id=3 WHERE id=2;
         UPDATE audio_files SET missing=0 WHERE id=3;
         UPDATE audio_files SET user_tags='metal',user_tags_tokenized='metal' WHERE id=4;
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
}

#[test]
fn repeated_cross_query_facets_match_the_sql_oracle_for_filters_and_visibility() {
    let (_directory, database, provider) = fixture();
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
}
