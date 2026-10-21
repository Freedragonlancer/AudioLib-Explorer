// crates/audiolib-db/tests/search_sorted.rs
//! Broad alternative ordering retains native NULL/tie/visibility/page semantics.
use audiolib_core::{DataProvider, FileVisibility, SearchFilters, SearchRequest, SortOrder};
use audiolib_db::{SqliteProvider, import_legacy};
use rusqlite::{Connection, params};

fn oracle(conn: &Connection, request: &SearchRequest) -> (u64, Vec<i64>) {
    let visibility = match request.filters.visibility {
        FileVisibility::Present => "af.missing=0",
        FileVisibility::Missing => "af.missing=1",
        FileVisibility::All => "1=1",
    };
    // All fixture matches contain the complete raw token explosion. This oracle
    // bypasses the fragment compiler and its new ordered-page strategy entirely.
    let matched = format!(
        "FROM audio_search CROSS JOIN audio_files af ON af.id=audio_search.rowid
        WHERE audio_search MATCH 'explosion' AND {visibility}"
    );
    let total = conn
        .query_row(&format!("SELECT COUNT(*) {matched}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    let total = u64::try_from(total).unwrap();
    let direction = if request.descending { "DESC" } else { "ASC" };
    let column = match request.order_by {
        SortOrder::Duration => "af.duration",
        SortOrder::SampleRate => "af.sample_rate",
        SortOrder::Modified => "af.modified_at",
        SortOrder::Channels => "af.channels",
        _ => panic!("unsupported column oracle ordering"),
    };
    let ids = if request.offset as u64 >= total {
        Vec::new()
    } else {
        conn.prepare(&format!(
            "SELECT af.id {matched} ORDER BY {column} {direction},af.id ASC LIMIT ?1 OFFSET ?2"
        ))
        .unwrap()
        .query_map(
            params![request.limit as i64, request.offset as i64],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    };
    (total, ids)
}

#[test]
fn broad_column_pages_match_native_order_before_and_after_visibility_changes() {
    let directory = tempfile::tempdir().unwrap();
    let legacy = directory.path().join("nullable-legacy.db");
    let path = directory.path().join("broad-columns.db");
    // Supported legacy imports preserve nullable visibility storage. A fresh
    // current table deliberately has a NOT NULL constraint instead.
    let source = Connection::open(&legacy).unwrap();
    audiolib_db::register_library_functions(&source).unwrap();
    source
        .execute_batch(
            "CREATE TABLE audio_files(id INTEGER PRIMARY KEY,file_path TEXT NOT NULL UNIQUE,
             filename TEXT NOT NULL,duration REAL,sample_rate INTEGER,channels INTEGER,
             bit_depth INTEGER,modified_at TEXT,missing INTEGER);
             CREATE TABLE folders(id INTEGER PRIMARY KEY,folder_path TEXT NOT NULL UNIQUE,
             folder_name TEXT NOT NULL,parent_id INTEGER);",
        )
        .unwrap();
    drop(source);
    import_legacy(&legacy, &path).unwrap();
    let provider = SqliteProvider::open_existing(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    audiolib_db::register_library_functions(&conn).unwrap();
    conn.execute_batch(
        "WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM n WHERE id<32768)
        INSERT INTO audio_files(id,file_path,filename,duration,sample_rate,channels,
            bit_depth,modified_at,missing)
        SELECT id,printf('root/%d.wav',id),'explosion.wav',
            CASE WHEN id%13=0 THEN NULL WHEN id%7=0 THEN 0 ELSE (id%17)*0.25 END,
            CASE WHEN id%11=0 THEN NULL WHEN id%7=0 THEN 0 WHEN id%3=0 THEN 48000 ELSE 96000 END,
            CASE WHEN id%11=0 THEN NULL WHEN id%7=0 THEN 0 WHEN id%3=0 THEN 1 ELSE 2 END,
            CASE WHEN id%17=0 THEN NULL WHEN id%2=0 THEN 16 ELSE 24 END,
            CASE WHEN id%13=0 THEN NULL ELSE printf('2026-10-%02dT00:00:00Z',1+id%7) END,0 FROM n;
        INSERT INTO audio_files(id,file_path,filename,duration,sample_rate,channels,
            bit_depth,modified_at,missing)
            VALUES(-7,'root/negative.wav','explosion.wav',NULL,NULL,NULL,NULL,NULL,0),
            (40000,'root/unmatched.wav','wind.wav',99999,192000,128,32,'2027-01-01T00:00:00Z',0);",
    )
    .unwrap();
    // Full signed rowid values must survive JSON/IN membership and stable ties.
    for (id, rate, channels, modified) in [
        (i64::MIN, None, None, None),
        (0, Some(48000), Some(1), Some("2026-10-01T00:00:00Z")),
        (i64::MAX, Some(96000), Some(2), Some("2026-10-02T00:00:00Z")),
    ] {
        conn.execute(
            "INSERT INTO audio_files(id,file_path,filename,duration,sample_rate,channels,
                bit_depth,modified_at,missing)
             VALUES(?1,?2,'explosion.wav',0,?3,?4,24,?5,0)",
            params![
                id,
                format!("root/signed-{id}.wav"),
                rate,
                channels,
                modified
            ],
        )
        .unwrap();
    }
    provider
        .maintain_library(audiolib_core::MaintenanceAction::RebuildSearch)
        .unwrap();
    for changed_visibility in [false, true] {
        if changed_visibility {
            conn.execute_batch("UPDATE audio_files SET missing=CASE id WHEN 1 THEN NULL WHEN 2 THEN 2 WHEN 3 THEN -1 ELSE 1 END WHERE id IN(1,2,3,4)").unwrap();
        }
        for order_by in [
            SortOrder::Duration,
            SortOrder::SampleRate,
            SortOrder::Modified,
            SortOrder::Channels,
        ] {
            for visibility in [
                FileVisibility::Present,
                FileVisibility::All,
                FileVisibility::Missing,
            ] {
                for descending in [false, true] {
                    for offset in [0, 7, 32760, usize::MAX] {
                        let request = SearchRequest {
                            query: "on".into(),
                            order_by,
                            descending,
                            limit: 7,
                            offset,
                            filters: SearchFilters {
                                visibility,
                                ..Default::default()
                            },
                            ..Default::default()
                        };
                        let (total, ids) = oracle(&conn, &request);
                        let page = provider.search(&request).unwrap();
                        assert_eq!(page.total_count, total);
                        assert_eq!(
                            page.results.iter().map(|file| file.id).collect::<Vec<_>>(),
                            ids,
                            "changed={changed_visibility},order_by={order_by:?},visibility={visibility:?},descending={descending},offset={offset}"
                        );
                        assert_eq!(page.returned_count, page.results.len());
                        assert!(!page.cache_hit);
                    }
                }
            }
        }
    }
    // Present plus an all-present snapshot makes every fast path eligible before
    // each negative index check; All or stale missing flags would mask bad guards.
    conn.execute_batch("UPDATE audio_files SET missing=0")
        .unwrap();
    for (order_by, index, columns, expected_partial) in [
        (SortOrder::Duration, "idx_duration", "duration", false),
        (
            SortOrder::SampleRate,
            "idx_sample_rate",
            "sample_rate",
            false,
        ),
        (
            SortOrder::Modified,
            "idx_audio_visible_modified",
            "modified_at,id",
            true,
        ),
        (
            SortOrder::Channels,
            "idx_audio_visible_format",
            "bit_depth,channels",
            true,
        ),
    ] {
        // Missing, same-name wrong partial predicate, and wrong-column optional
        // indexes must all keep the complete fallback. The two-column format
        // index also needs both retained columns, not merely its final sort key.
        let invalid_predicate = if expected_partial { 1 } else { 0 };
        let mut replacements = vec![
            format!("DROP INDEX {index}"),
            format!(
                "CREATE INDEX {index} ON audio_files({columns}) WHERE missing={invalid_predicate}"
            ),
            format!("DROP INDEX {index}; CREATE INDEX {index} ON audio_files(filename)"),
        ];
        if expected_partial {
            let incomplete_columns = if order_by == SortOrder::Channels {
                "channels"
            } else {
                "modified_at"
            };
            replacements.push(format!(
                "DROP INDEX {index}; CREATE INDEX {index} ON audio_files({incomplete_columns}) WHERE missing=0"
            ));
        }
        for replacement in replacements {
            conn.execute_batch(&replacement).unwrap();
            for descending in [false, true] {
                let request = SearchRequest {
                    query: "on".into(),
                    order_by,
                    descending,
                    offset: 7,
                    limit: 7,
                    filters: SearchFilters {
                        visibility: FileVisibility::Present,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let (total, ids) = oracle(&conn, &request);
                let page = provider.search(&request).unwrap();
                assert_eq!(page.total_count, total);
                assert_eq!(
                    page.results.iter().map(|file| file.id).collect::<Vec<_>>(),
                    ids,
                    "index={index},replacement={replacement},descending={descending}"
                );
                assert_eq!(page.returned_count, ids.len());
            }
        }
    }
}
