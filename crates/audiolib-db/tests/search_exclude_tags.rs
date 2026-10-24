// crates/audiolib-db/tests/search_exclude_tags.rs
//! Exclude-only tag filters keep the exact totals, ordered pages and directory facets
//! of the original full-scan statements, on writable and read-only providers, and
//! a query keeps its ranking order with exactly the passing files.
use audiolib_core::{DataProvider, FileVisibility, SearchFilters, SearchRequest, SortOrder};
use audiolib_db::{SqliteProvider, register_library_functions};
use rusqlite::{Connection, params, params_from_iter, types::Value};
use std::collections::{BTreeMap, HashSet};

const RECORDS: i64 = 6_000;

fn fixture() -> (tempfile::TempDir, SqliteProvider, Connection) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("exclude.db");
    let provider = SqliteProvider::open(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    register_library_functions(&conn).unwrap();
    // root(1) -> a(2) -> a/deep(4), root(1) -> b(3)
    conn.execute_batch(
        "INSERT INTO folders(id,folder_path,folder_name,parent_id,depth) VALUES
           (1,'root','root',NULL,0),(2,'root/a','a',1,1),(3,'root/b','b',1,1),
           (4,'root/a/deep','deep',2,2);
         INSERT INTO folder_closure(ancestor_id,descendant_id,depth) VALUES
           (1,1,0),(2,2,0),(3,3,0),(4,4,0),(1,2,1),(1,3,1),(2,4,1),(1,4,2);",
    )
    .unwrap();
    let annotations = [
        None,
        Some(""),
        Some("   "),
        Some("\t"),
        Some("Engine, Exterior"),
        Some("door"),
        Some("ENGINE rain"),
        Some("Explosion metal outdoor"),
        Some("Ünïcode Tür"),
    ];
    let keywords = ["", "Engine; Rain", "Interior", "Explosion", "door, ÜNÏCODE"];
    let mut insert = conn
        .prepare(
            "INSERT INTO audio_files(id,file_path,filename,folder_id,user_tags,metadata_json,
             missing,duration,sample_rate,source_tags_text)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,COALESCE(audiolib_source_tags_key(?6),''))",
        )
        .unwrap();
    conn.execute_batch("BEGIN").unwrap();
    for id in 1..=RECORDS {
        let index = usize::try_from(id).unwrap();
        let keyword = keywords[(index / 3) % keywords.len()];
        let metadata = if keyword.is_empty() {
            "{}".to_string()
        } else {
            serde_json::json!({"wav_metadata": {"fields": {"keywords": keyword}}}).to_string()
        };
        // Many equal durations (and NULLs) exercise the id tie-breaker in both directions.
        let duration = (id % 29 != 0).then_some((id % 37) as f64 * 0.25);
        insert
            .execute(params![
                id,
                format!("root/{id}.wav"),
                format!(
                    "{}_{:04}.WAV",
                    ["alpha", "Bravo", "charlie"][index % 3],
                    (id * 7_919) % RECORDS
                ),
                [1, 2, 3, 4][index % 4],
                annotations[index % annotations.len()],
                metadata,
                i64::from(id % 11 == 0),
                duration,
                [48_000, 96_000, 44_100][index % 3],
            ])
            .unwrap();
    }
    conn.execute_batch("COMMIT; ANALYZE;").unwrap();
    drop(insert);
    (directory, provider, conn)
}

/// The original exclude-only statements: one full COUNT and one ordered page, both
/// testing every visible row's tags. Facets count the same rows per direct folder.
struct Oracle<'a> {
    conn: &'a Connection,
}

impl Oracle<'_> {
    fn scope(request: &SearchRequest) -> (String, Vec<Value>) {
        let terms = request
            .filters
            .exclude_tags
            .iter()
            .map(|term| (true, term.to_lowercase()))
            .collect::<Vec<_>>();
        let mut conditions = vec![
            match request.filters.visibility {
                FileVisibility::Present => "af.missing=0",
                FileVisibility::Missing => "af.missing=1",
                FileVisibility::All => "1=1",
            }
            .to_string(),
            "((COALESCE(trim(af.user_tags),'')='' AND af.source_tags_text='')
              OR audiolib_tag_terms(af.user_tags,af.source_tags_text,?)=1)"
                .to_string(),
        ];
        let mut values = vec![Value::Text(serde_json::to_string(&terms).unwrap())];
        if let Some(folder) = request.folder_id {
            conditions.push(if request.include_subfolders {
                "af.folder_id IN (SELECT descendant_id FROM folder_closure WHERE ancestor_id=?)"
                    .into()
            } else {
                "af.folder_id=?".into()
            });
            values.push(Value::Integer(folder));
        }
        (conditions.join(" AND "), values)
    }

    fn page(&self, request: &SearchRequest) -> (u64, Vec<i64>) {
        let (conditions, mut values) = Self::scope(request);
        let total: i64 = self
            .conn
            .query_row(
                &format!("SELECT COUNT(*) FROM audio_files af WHERE {conditions}"),
                params_from_iter(&values),
                |row| row.get(0),
            )
            .unwrap();
        let (order, direction) = match request.order_by {
            SortOrder::Duration => ("af.duration", request.descending),
            SortOrder::SampleRate => ("af.sample_rate", request.descending),
            SortOrder::Filename => ("af.filename COLLATE NOCASE", request.descending),
            SortOrder::Relevance => ("af.filename COLLATE NOCASE", false),
            other => panic!("no oracle for {other:?}"),
        };
        let direction = if direction { "DESC" } else { "ASC" };
        values.push(Value::Integer(request.limit as i64));
        values.push(Value::Integer(request.offset as i64));
        let ids = self
            .conn
            .prepare(&format!(
                "SELECT af.id FROM audio_files af WHERE {conditions}
                 ORDER BY {order} {direction},af.id ASC LIMIT ? OFFSET ?"
            ))
            .unwrap()
            .query_map(params_from_iter(&values), |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<i64>>>()
            .unwrap();
        (u64::try_from(total).unwrap(), ids)
    }

    /// Rolled-up folder counts, independent of the requested folder scope.
    fn facets(&self, request: &SearchRequest) -> BTreeMap<i64, u64> {
        let unscoped = SearchRequest {
            folder_id: None,
            ..request.clone()
        };
        let (conditions, values) = Self::scope(&unscoped);
        let direct = self
            .conn
            .prepare(&format!(
                "SELECT af.folder_id,COUNT(*) FROM audio_files af WHERE {conditions}
                 GROUP BY af.folder_id"
            ))
            .unwrap()
            .query_map(params_from_iter(&values), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let mut totals = BTreeMap::new();
        for (folder, matches) in direct {
            let ancestors = self
                .conn
                .prepare("SELECT ancestor_id FROM folder_closure WHERE descendant_id=?1")
                .unwrap()
                .query_map([folder], |row| row.get::<_, i64>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            for ancestor in ancestors {
                *totals.entry(ancestor).or_default() += u64::try_from(matches).unwrap();
            }
        }
        totals.retain(|_, count| *count > 0);
        totals
    }
}

fn excluding(terms: &[&str]) -> SearchFilters {
    SearchFilters {
        exclude_tags: terms.iter().map(|term| (*term).into()).collect(),
        ..Default::default()
    }
}

fn page_ids(provider: &SqliteProvider, request: &SearchRequest) -> (u64, Vec<i64>) {
    let page = provider.search(request).unwrap();
    assert_eq!(page.returned_count, page.results.len());
    (
        page.total_count,
        page.results.iter().map(|file| file.id).collect(),
    )
}

fn facet_counts(provider: &SqliteProvider, request: &SearchRequest) -> BTreeMap<i64, u64> {
    provider
        .search_folders(request)
        .unwrap()
        .into_iter()
        .map(|folder| (folder.id, folder.file_count))
        .collect()
}

const TERM_SETS: [&[&str]; 6] = [
    &["explosion"],
    &["Engine", "door"],
    &["rain", "interior", "TÜR"],
    &["ünïcode"],
    &["not-a-tag"],
    // A term matching every tagged file leaves only the untagged ones.
    &["e", "o", "i", "a", "u", "r"],
];

const ORDERS: [(SortOrder, bool); 6] = [
    (SortOrder::Duration, false),
    (SortOrder::Duration, true),
    (SortOrder::SampleRate, true),
    (SortOrder::Filename, false),
    (SortOrder::Filename, true),
    (SortOrder::Relevance, false),
];

#[test]
fn exclude_only_pages_totals_and_facets_match_the_full_scan_statements() {
    let (_directory, provider, conn) = fixture();
    let oracle = Oracle { conn: &conn };
    let read_only = SqliteProvider::open_read_only(provider.database_path()).unwrap();
    for terms in TERM_SETS {
        let filters = excluding(terms);
        let base = SearchRequest {
            filters: filters.clone(),
            ..Default::default()
        };
        let facets = oracle.facets(&base);
        assert_eq!(facet_counts(&provider, &base), facets, "{terms:?}");
        assert_eq!(facet_counts(&read_only, &base), facets, "{terms:?}");
        for scope in [None, Some((2, true)), Some((2, false)), Some((4, true))] {
            for (order_by, descending) in ORDERS {
                let request = SearchRequest {
                    folder_id: scope.map(|(folder, _)| folder),
                    include_subfolders: scope.is_none_or(|(_, subfolders)| subfolders),
                    order_by,
                    descending,
                    ..base.clone()
                };
                let (total, _) = oracle.page(&request);
                let offsets = [0, 1, 199, total.saturating_sub(5), total, total + 10];
                for offset in offsets {
                    for limit in [200, 37] {
                        let request = SearchRequest {
                            offset: usize::try_from(offset).unwrap(),
                            limit,
                            ..request.clone()
                        };
                        let expected = oracle.page(&request);
                        let context = format!(
                            "{terms:?} {scope:?} {order_by:?} desc={descending} offset={offset} limit={limit}"
                        );
                        assert_eq!(page_ids(&provider, &request), expected, "{context}");
                        assert_eq!(page_ids(&read_only, &request), expected, "{context}");
                    }
                }
            }
        }
    }
}

#[test]
fn exclude_only_duration_pages_concatenate_to_the_complete_stable_order() {
    let (_directory, provider, conn) = fixture();
    let oracle = Oracle { conn: &conn };
    for terms in [&["explosion"][..], &["Engine", "door"], &["not-a-tag"]] {
        for descending in [false, true] {
            let request = SearchRequest {
                filters: excluding(terms),
                order_by: SortOrder::Duration,
                descending,
                limit: 1_000_000,
                ..Default::default()
            };
            let (total, complete) = oracle.page(&request);
            assert_eq!(complete.len() as u64, total);
            let mut paged = Vec::new();
            for offset in (0..complete.len() + 97).step_by(97) {
                let (page_total, ids) = page_ids(
                    &provider,
                    &SearchRequest {
                        offset,
                        limit: 97,
                        ..request.clone()
                    },
                );
                assert_eq!(page_total, total, "{terms:?} desc={descending}");
                paged.extend(ids);
            }
            assert_eq!(paged, complete, "{terms:?} desc={descending}");
        }
    }
}

#[test]
fn other_visibilities_with_excluded_tags_keep_their_statements() {
    let (_directory, provider, conn) = fixture();
    let oracle = Oracle { conn: &conn };
    for visibility in [FileVisibility::Missing, FileVisibility::All] {
        for (order_by, descending) in [(SortOrder::Duration, false), (SortOrder::Duration, true)] {
            let request = SearchRequest {
                filters: SearchFilters {
                    visibility,
                    ..excluding(&["explosion", "door"])
                },
                order_by,
                descending,
                offset: 3,
                ..Default::default()
            };
            assert_eq!(page_ids(&provider, &request), oracle.page(&request));
            assert_eq!(facet_counts(&provider, &request), oracle.facets(&request));
        }
    }
}

/// Every file matching `query` in its ranking order, whatever its visibility or tags.
fn ranking(provider: &SqliteProvider, query: &str) -> Vec<i64> {
    let request = SearchRequest {
        query: query.into(),
        filters: SearchFilters {
            visibility: FileVisibility::All,
            ..Default::default()
        },
        limit: 10_000,
        ..Default::default()
    };
    let (total, ids) = page_ids(provider, &request);
    assert_eq!(total as usize, ids.len());
    ids
}

#[test]
fn query_rankings_keep_their_order_with_exactly_the_passing_files() {
    let (_directory, provider, conn) = fixture();
    let oracle = Oracle { conn: &conn };
    let folders = conn
        .prepare("SELECT id,folder_id FROM audio_files")
        .unwrap()
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()
        .unwrap();
    let closure = conn
        .prepare("SELECT ancestor_id,descendant_id FROM folder_closure")
        .unwrap()
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    for query in ["bravo", "wav"] {
        let ranked = ranking(&provider, query);
        for terms in TERM_SETS {
            let filters = excluding(terms);
            let (_, passing) = oracle.page(&SearchRequest {
                filters: filters.clone(),
                order_by: SortOrder::Duration,
                limit: 1_000_000,
                ..Default::default()
            });
            let passing = passing.into_iter().collect::<HashSet<_>>();
            let request = SearchRequest {
                query: query.into(),
                filters,
                ..Default::default()
            };
            // Facets first on a new provider, before any ranking is cached.
            let mut facets = BTreeMap::new();
            let expected = ranked
                .iter()
                .copied()
                .filter(|id| passing.contains(id))
                .collect::<Vec<_>>();
            for id in &expected {
                for (ancestor, descendant) in &closure {
                    if *descendant == folders[id] {
                        *facets.entry(*ancestor).or_insert(0u64) += 1;
                    }
                }
            }
            let fresh = SqliteProvider::open_read_only(provider.database_path()).unwrap();
            assert_eq!(facet_counts(&fresh, &request), facets, "{query} {terms:?}");
            assert_eq!(
                facet_counts(&provider, &request),
                facets,
                "{query} {terms:?}"
            );
            let total = expected.len();
            for (offset, limit) in [
                (0, 200),
                (37, 37),
                (total.saturating_sub(3), 200),
                (total + 5, 200),
            ] {
                let page = SearchRequest {
                    offset,
                    limit,
                    ..request.clone()
                };
                let wanted = expected.iter().skip(offset).take(limit).copied().collect();
                let context = format!("{query} {terms:?} offset={offset} limit={limit}");
                assert_eq!(
                    page_ids(&provider, &page),
                    (total as u64, wanted),
                    "{context}"
                );
            }
            // A folder scope keeps the shared filter membership.
            let scoped = SearchRequest {
                folder_id: Some(2),
                ..request.clone()
            };
            let wanted = expected
                .iter()
                .copied()
                .filter(|id| matches!(folders[id], 2 | 4))
                .collect::<Vec<_>>();
            let (scoped_total, scoped_ids) = page_ids(&provider, &scoped);
            assert_eq!(scoped_total as usize, wanted.len(), "{query} {terms:?}");
            assert_eq!(scoped_ids, wanted.into_iter().take(200).collect::<Vec<_>>());
        }
    }
}
