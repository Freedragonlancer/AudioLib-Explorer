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
    let ids = if request.offset as u64 >= total {
        Vec::new()
    } else {
        conn.prepare(&format!(
            "SELECT af.id {matched} ORDER BY af.duration {direction},af.id ASC LIMIT ?1 OFFSET ?2"
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
fn broad_duration_pages_match_native_order_before_and_after_visibility_changes() {
    let directory = tempfile::tempdir().unwrap();
    let legacy = directory.path().join("nullable-legacy.db");
    let path = directory.path().join("broad-duration.db");
    // Supported legacy imports preserve nullable visibility storage. A fresh
    // current table deliberately has a NOT NULL constraint instead.
    let source = Connection::open(&legacy).unwrap();
    audiolib_db::register_library_functions(&source).unwrap();
    source
        .execute_batch(
            "CREATE TABLE audio_files(id INTEGER PRIMARY KEY,file_path TEXT NOT NULL UNIQUE,
             filename TEXT NOT NULL,duration REAL,missing INTEGER);
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
        INSERT INTO audio_files(id,file_path,filename,duration,missing)
        SELECT id,printf('root/%d.wav',id),'explosion.wav',
            CASE WHEN id%13=0 THEN NULL WHEN id%7=0 THEN 0 ELSE (id%17)*0.25 END,0 FROM n;
        INSERT INTO audio_files(id,file_path,filename,duration,missing)
            VALUES(-7,'root/negative.wav','explosion.wav',NULL,0),
            (40000,'root/unmatched.wav','wind.wav',99999,0);",
    )
    .unwrap();
    provider
        .maintain_library(audiolib_core::MaintenanceAction::RebuildSearch)
        .unwrap();
    for changed_visibility in [false, true] {
        if changed_visibility {
            conn.execute_batch("UPDATE audio_files SET missing=CASE id WHEN 1 THEN NULL WHEN 2 THEN 2 WHEN 3 THEN -1 ELSE 1 END WHERE id IN(1,2,3,4)").unwrap();
        }
        for visibility in [
            FileVisibility::Present,
            FileVisibility::All,
            FileVisibility::Missing,
        ] {
            for descending in [false, true] {
                for offset in [0, 7, 32760, usize::MAX] {
                    let request = SearchRequest {
                        query: "on".into(),
                        order_by: SortOrder::Duration,
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
                        "changed={changed_visibility},visibility={visibility:?},descending={descending},offset={offset}"
                    );
                    assert_eq!(page.returned_count, page.results.len());
                    assert!(!page.cache_hit);
                }
            }
        }
    }
    // An otherwise readable snapshot may lack this optional performance index.
    conn.execute_batch("DROP INDEX idx_duration").unwrap();
    let request = SearchRequest {
        query: "on".into(),
        order_by: SortOrder::Duration,
        descending: true,
        filters: SearchFilters {
            visibility: FileVisibility::All,
            ..Default::default()
        },
        ..Default::default()
    };
    let (total, ids) = oracle(&conn, &request);
    let page = provider.search(&request).unwrap();
    assert_eq!(page.total_count, total);
    assert_eq!(
        page.results.iter().map(|file| file.id).collect::<Vec<_>>(),
        ids
    );
    // A same-name partial index also cannot satisfy an All-visibility scan.
    conn.execute_batch("CREATE INDEX idx_duration ON audio_files(duration) WHERE missing=0")
        .unwrap();
    let page = provider.search(&request).unwrap();
    assert_eq!(page.total_count, total);
    assert_eq!(
        page.results.iter().map(|file| file.id).collect::<Vec<_>>(),
        ids
    );
}
