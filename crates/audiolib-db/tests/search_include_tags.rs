// crates/audiolib-db/tests/search_include_tags.rs
//! Include tag filters keep the exact totals and ordered pages of a statement that
//! tests every visible row, for sparse and dense matches, with and without ANALYZE
//! statistics, on writable and read-only providers.
use audiolib_core::{DataProvider, SearchFilters, SearchRequest, SortOrder};
use audiolib_db::{SqliteProvider, register_library_functions};
use rusqlite::{Connection, params, params_from_iter, types::Value};

const RECORDS: i64 = 6_000;

fn fixture(analyze: bool) -> (tempfile::TempDir, SqliteProvider, Connection) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("include.db");
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
             missing,duration,sample_rate,modified_at,source_tags_text)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,COALESCE(audiolib_source_tags_key(?6),''))",
        )
        .unwrap();
    conn.execute_batch("BEGIN").unwrap();
    for id in 1..=RECORDS {
        let index = usize::try_from(id).unwrap();
        // 15 sparse "whistle" files, half of them only in source tags.
        let (annotation, keyword) = if id % 400 == 0 {
            (Some("Whistle"), "")
        } else if id % 400 == 200 {
            (None, "whistle, Interior")
        } else {
            (
                annotations[index % annotations.len()],
                keywords[(index / 3) % keywords.len()],
            )
        };
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
                annotation,
                metadata,
                i64::from(id % 11 == 0),
                duration,
                [48_000, 96_000, 44_100][index % 3],
                format!("2026-01-{:02}T00:00:00", id % 28 + 1),
            ])
            .unwrap();
    }
    conn.execute_batch("COMMIT").unwrap();
    if analyze {
        conn.execute_batch("ANALYZE").unwrap();
    }
    drop(insert);
    (directory, provider, conn)
}

/// Every visible row tests the terms; no index can skip a file.
fn oracle(conn: &Connection, request: &SearchRequest) -> (u64, Vec<i64>) {
    let filters = &request.filters;
    let terms = filters
        .tag_contains
        .iter()
        .chain(&filters.include_tags)
        .map(|term| (false, term.to_lowercase()))
        .chain(
            filters
                .exclude_tags
                .iter()
                .map(|term| (true, term.to_lowercase())),
        )
        .collect::<Vec<_>>();
    let mut conditions = vec![
        "af.missing=0".to_string(),
        "audiolib_tag_terms(af.user_tags,af.source_tags_text,?)=1".to_string(),
    ];
    let mut values = vec![Value::Text(serde_json::to_string(&terms).unwrap())];
    if let Some(rate) = filters.sample_rate {
        conditions.push("af.sample_rate=?".into());
        values.push(Value::Integer(i64::from(rate)));
    }
    if let Some(folder) = request.folder_id {
        conditions.push(if request.include_subfolders {
            "af.folder_id IN (SELECT descendant_id FROM folder_closure WHERE ancestor_id=?)".into()
        } else {
            "af.folder_id=?".into()
        });
        values.push(Value::Integer(folder));
    }
    let conditions = conditions.join(" AND ");
    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM audio_files af NOT INDEXED WHERE {conditions}"),
            params_from_iter(&values),
            |row| row.get(0),
        )
        .unwrap();
    let (order, descending) = match request.order_by {
        SortOrder::Duration => ("af.duration", request.descending),
        SortOrder::SampleRate => ("af.sample_rate", request.descending),
        SortOrder::Modified => ("af.modified_at", request.descending),
        SortOrder::Tags => (
            "COALESCE(audiolib_user_tags_key(af.user_tags),af.source_tags_text) COLLATE NOCASE",
            request.descending,
        ),
        SortOrder::Filename => ("af.filename COLLATE NOCASE", request.descending),
        SortOrder::Relevance => ("af.filename COLLATE NOCASE", false),
        other => panic!("no oracle for {other:?}"),
    };
    let direction = if descending { "DESC" } else { "ASC" };
    values.push(Value::Integer(request.limit as i64));
    values.push(Value::Integer(request.offset as i64));
    let ids = conn
        .prepare(&format!(
            "SELECT af.id FROM audio_files af NOT INDEXED WHERE {conditions}
             ORDER BY {order} {direction},af.id ASC LIMIT ? OFFSET ?"
        ))
        .unwrap()
        .query_map(params_from_iter(&values), |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<i64>>>()
        .unwrap();
    (u64::try_from(total).unwrap(), ids)
}

fn page_ids(provider: &SqliteProvider, request: &SearchRequest) -> (u64, Vec<i64>) {
    let page = provider.search(request).unwrap();
    assert_eq!(page.returned_count, page.results.len());
    (
        page.total_count,
        page.results.iter().map(|file| file.id).collect(),
    )
}

fn filters(contains: Option<&str>, include: &[&str], exclude: &[&str]) -> SearchFilters {
    SearchFilters {
        tag_contains: contains.map(Into::into),
        include_tags: include.iter().map(|term| (*term).into()).collect(),
        exclude_tags: exclude.iter().map(|term| (*term).into()).collect(),
        ..Default::default()
    }
}

const ORDERS: [(SortOrder, bool); 8] = [
    (SortOrder::Duration, false),
    (SortOrder::Duration, true),
    (SortOrder::SampleRate, true),
    (SortOrder::Modified, true),
    (SortOrder::Tags, false),
    (SortOrder::Filename, false),
    (SortOrder::Filename, true),
    (SortOrder::Relevance, false),
];

#[test]
fn include_pages_and_totals_match_a_statement_testing_every_row() {
    for analyze in [false, true] {
        let (_directory, provider, conn) = fixture(analyze);
        let read_only = SqliteProvider::open_read_only(provider.database_path()).unwrap();
        let cases = [
            // Sparse: 15 of 6 000 files, from library and from source tags.
            filters(Some("whistle"), &[], &[]),
            filters(None, &["WHISTLE"], &["interior"]),
            // Dense: about a third of the files.
            filters(Some("engine"), &[], &[]),
            filters(None, &["engine", "rain"], &[]),
            filters(None, &["e"], &["door", "ünïcode"]),
            SearchFilters {
                sample_rate: Some(48_000),
                ..filters(Some("Engine"), &[], &[])
            },
            filters(None, &["ünïcode"], &[]),
            filters(None, &["not-a-tag"], &[]),
        ];
        for filters in cases {
            for scope in [None, Some((2, true)), Some((3, false))] {
                for (order_by, descending) in ORDERS {
                    let request = SearchRequest {
                        filters: filters.clone(),
                        folder_id: scope.map(|(folder, _)| folder),
                        include_subfolders: scope.is_none_or(|(_, subfolders)| subfolders),
                        order_by,
                        descending,
                        ..Default::default()
                    };
                    let (total, _) = oracle(&conn, &request);
                    for (offset, limit) in [
                        (0, 200),
                        (1, 37),
                        (199, 37),
                        (total.saturating_sub(5), 37),
                        (total + 10, 37),
                    ] {
                        let request = SearchRequest {
                            offset: usize::try_from(offset).unwrap(),
                            limit,
                            ..request.clone()
                        };
                        let expected = oracle(&conn, &request);
                        let context = format!(
                            "analyze={analyze} {:?} {scope:?} {order_by:?} desc={descending} offset={offset} limit={limit}",
                            request.filters
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
fn include_duration_pages_concatenate_to_the_complete_stable_order() {
    let (_directory, provider, conn) = fixture(false);
    for filters in [
        filters(Some("whistle"), &[], &[]),
        filters(Some("engine"), &[], &[]),
        filters(None, &["e"], &["door"]),
    ] {
        for descending in [false, true] {
            let request = SearchRequest {
                filters: filters.clone(),
                order_by: SortOrder::Duration,
                descending,
                limit: 1_000_000,
                ..Default::default()
            };
            let (total, complete) = oracle(&conn, &request);
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
                assert_eq!(page_total, total);
                paged.extend(ids);
            }
            assert_eq!(paged, complete, "{filters:?} desc={descending}");
        }
    }
}
