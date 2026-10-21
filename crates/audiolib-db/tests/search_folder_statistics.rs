// crates/audiolib-db/tests/search_folder_statistics.rs
//! Directory facets remain exact after growth leaves tiny-library ANALYZE statistics.
use audiolib_core::{DataProvider, Error, FileVisibility, SearchFilters, SearchRequest, SortOrder};
use audiolib_db::SqliteProvider;
use rusqlite::{Connection, params};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

type Statistics = Vec<(String, Option<String>, String)>;

struct Input {
    id: i64,
    ancestors: Vec<i64>,
    term: &'static str,
    sample_rate: u32,
    favorite: bool,
    missing: bool,
}

struct Fixture {
    // Keep the directory alive until its provider has closed SQLite connections.
    provider: SqliteProvider,
    inputs: Vec<Input>,
    initial_statistics: Statistics,
    _directory: tempfile::TempDir,
}

fn statistics(conn: &Connection) -> Statistics {
    conn.prepare(
        "SELECT tbl,idx,stat FROM sqlite_stat1
         WHERE tbl IN ('folders','folder_closure','audio_files') ORDER BY tbl,idx",
    )
    .unwrap()
    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("stale-folder-statistics.db");
    let provider = SqliteProvider::open(&database).unwrap();
    let mut conn = Connection::open(&database).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO folders(id,folder_path,folder_name) VALUES(1,'synthetic','seed');
         INSERT INTO folder_closure VALUES(1,1,0);
         INSERT INTO audio_files(id,file_path,filename,folder_id,sample_rate,favorite,user_tags,missing) VALUES(1,'synthetic/seed.wav','seed.wav',1,48000,1,'metal',0);
         ANALYZE;",
    )
    .unwrap();
    let initial_statistics = statistics(&conn);
    assert!(!initial_statistics.is_empty());
    assert!(initial_statistics.iter().any(|(table, _, stat)| {
        table == "folder_closure" && stat.split_whitespace().next() == Some("1")
    }));
    let mut inputs = vec![Input {
        id: 1,
        ancestors: vec![1],
        term: "metal",
        sample_rate: 48_000,
        favorite: true,
        missing: false,
    }];
    let tx = conn.transaction().unwrap();
    for branch in 0..20_i64 {
        let branch_id = branch + 2;
        let branch_path = format!("synthetic/branch-{branch:02}");
        tx.execute(
            "INSERT INTO folders(id,folder_path,folder_name,parent_id)
             VALUES(?1,?2,?3,1)",
            params![branch_id, branch_path, format!("branch-{branch:02}")],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO folder_closure VALUES(?1,?1,0),(1,?1,1)",
            [branch_id],
        )
        .unwrap();
        for leaf in 0..100_i64 {
            let index = branch * 100 + leaf;
            let folder_id = 100 + index;
            let id = index + 2;
            let folder_path = format!("{branch_path}/leaf-{leaf:03}");
            let term = if index % 3 == 0 { "metal" } else { "wood" };
            let sample_rate = if index % 2 == 0 { 48_000 } else { 96_000 };
            let favorite = index % 5 == 0;
            let missing = index % 11 == 0;
            tx.execute(
                "INSERT INTO folders(id,folder_path,folder_name,parent_id)
                 VALUES(?1,?2,?3,?4)",
                params![
                    folder_id,
                    folder_path,
                    format!("leaf-{index:04}"),
                    branch_id
                ],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO folder_closure VALUES(?1,?1,0),(?2,?1,1),(1,?1,2)",
                params![folder_id, branch_id],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO audio_files(id,file_path,filename,folder_id,sample_rate,favorite,user_tags,missing) VALUES(?1,?2,'input.wav',?3,?4,?5,?6,?7)",
                params![
                    id,
                    format!("{folder_path}/input.wav"),
                    folder_id,
                    sample_rate,
                    favorite,
                    term,
                    missing
                ],
            )
            .unwrap();
            inputs.push(Input {
                id,
                ancestors: vec![folder_id, branch_id, 1],
                term,
                sample_rate,
                favorite,
                missing,
            });
        }
    }
    tx.commit().unwrap();
    // Deliberately keep the statistics measured from exactly one folder and file.
    assert_eq!(statistics(&conn), initial_statistics);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM folders", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2021
    );
    Fixture {
        provider,
        inputs,
        initial_statistics,
        _directory: directory,
    }
}

fn matches(input: &Input, request: &SearchRequest) -> bool {
    (request.query.is_empty() || request.query == input.term)
        && match request.filters.visibility {
            FileVisibility::Present => !input.missing,
            FileVisibility::Missing => input.missing,
            FileVisibility::All => true,
        }
        && request
            .filters
            .sample_rate
            .is_none_or(|sample_rate| input.sample_rate == sample_rate)
        && (!request.filters.favorites_only || input.favorite)
}

fn expected_counts(inputs: &[Input], request: &SearchRequest) -> BTreeMap<i64, u64> {
    let mut counts = BTreeMap::new();
    for input in inputs.iter().filter(|input| matches(input, request)) {
        for ancestor in &input.ancestors {
            *counts.entry(*ancestor).or_default() += 1;
        }
    }
    counts
}

fn actual_counts(provider: &SqliteProvider, request: &SearchRequest) -> BTreeMap<i64, u64> {
    provider
        .search_folders(request)
        .unwrap()
        .into_iter()
        .map(|folder| (folder.id, folder.file_count))
        .collect()
}

#[test]
fn stale_single_folder_statistics_preserve_pages_and_exact_ancestor_rollup_after_growth() {
    let fixture = fixture();
    let requests = [
        SearchRequest::default(),
        SearchRequest {
            query: "metal".into(),
            ..Default::default()
        },
        SearchRequest {
            query: "wood".into(),
            filters: SearchFilters {
                sample_rate: Some(48_000),
                favorites_only: true,
                ..Default::default()
            },
            ..Default::default()
        },
        SearchRequest {
            filters: SearchFilters {
                sample_rate: Some(96_000),
                ..Default::default()
            },
            ..Default::default()
        },
        SearchRequest {
            query: "metal".into(),
            filters: SearchFilters {
                visibility: FileVisibility::Missing,
                ..Default::default()
            },
            ..Default::default()
        },
        SearchRequest {
            filters: SearchFilters {
                visibility: FileVisibility::All,
                ..Default::default()
            },
            ..Default::default()
        },
        SearchRequest {
            query: "notpresent".into(),
            ..Default::default()
        },
    ];
    for request in requests {
        let expected = expected_counts(&fixture.inputs, &request);
        assert_eq!(actual_counts(&fixture.provider, &request), expected);
        let page = fixture.provider.search(&request).unwrap();
        assert_eq!(page.total_count, *expected.get(&1).unwrap_or(&0));
        let matching_ids: BTreeSet<_> = fixture
            .inputs
            .iter()
            .filter(|input| matches(input, &request))
            .map(|input| input.id)
            .collect();
        assert_eq!(page.returned_count, request.limit.min(matching_ids.len()));
        assert!(
            page.results
                .iter()
                .all(|file| matching_ids.contains(&file.id))
        );
        // Facets intentionally ignore selected scope, page and sort for navigation.
        let scoped = SearchRequest {
            folder_id: Some(2),
            include_subfolders: false,
            offset: 1_000_000,
            limit: 1,
            order_by: SortOrder::Filename,
            descending: true,
            ..request
        };
        assert_eq!(actual_counts(&fixture.provider, &scoped), expected);
    }
    let conn = Connection::open(fixture.provider.database_path()).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    assert_eq!(statistics(&conn), fixture.initial_statistics);
}

#[test]
fn cancelled_facets_with_stale_statistics_exit_and_leave_later_reads_usable() {
    let fixture = fixture();
    let cancel = Arc::new(AtomicBool::new(true));
    for query in ["", "metal"] {
        let request = SearchRequest {
            query: query.into(),
            ..Default::default()
        };
        assert!(matches!(
            fixture
                .provider
                .search_folders_cancellable(&request, cancel.clone()),
            Err(Error::Cancelled)
        ));
        cancel.store(false, Ordering::Release);
        assert_eq!(
            actual_counts(&fixture.provider, &request),
            expected_counts(&fixture.inputs, &request)
        );
        cancel.store(true, Ordering::Release);
    }
    let conn = Connection::open(fixture.provider.database_path()).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    assert_eq!(statistics(&conn), fixture.initial_statistics);
}

#[test]
fn missing_rollup_index_upgrades_owned_readonly_copy_and_is_repaired_on_writable_open() {
    let Fixture {
        provider,
        inputs,
        _directory,
        ..
    } = fixture();
    let source = provider.database_path().to_path_buf();
    drop(provider);
    let conn = Connection::open(&source).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "DROP INDEX idx_closure_descendant;
         PRAGMA wal_checkpoint(TRUNCATE);
         PRAGMA journal_mode=DELETE;",
    )
    .unwrap();
    drop(conn);
    let original_bytes = std::fs::read(&source).unwrap();
    assert!(SqliteProvider::has_current_search_text(&source).unwrap());
    assert!(!SqliteProvider::has_current_search_state(&source).unwrap());
    let request = SearchRequest {
        query: "metal".into(),
        ..Default::default()
    };

    let readonly = SqliteProvider::open_read_only(&source).unwrap();
    assert_ne!(readonly.database_path(), source);
    assert!(SqliteProvider::has_current_search_state(readonly.database_path()).unwrap());
    assert_eq!(
        actual_counts(&readonly, &request),
        expected_counts(&inputs, &request)
    );
    assert_eq!(std::fs::read(&source).unwrap(), original_bytes);
    drop(readonly);

    let writable = SqliteProvider::open_existing(&source).unwrap();
    assert_eq!(writable.database_path(), source);
    assert!(SqliteProvider::has_current_search_state(&source).unwrap());
    let conn = Connection::open(&source).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type='index' AND name='idx_closure_descendant'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1
    );
    assert_eq!(
        actual_counts(&writable, &request),
        expected_counts(&inputs, &request)
    );
}
