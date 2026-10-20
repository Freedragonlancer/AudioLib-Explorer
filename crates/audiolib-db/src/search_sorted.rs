// crates/audiolib-db/src/search_sorted.rs
//! Native duration ordering over a bounded exact MATCH membership set.
use crate::{AUDIO_SELECT, Result, db_error, search_cache::MAX_RANKED_IDS};
use audiolib_core::{FileVisibility, SearchFilters, SearchRequest, SortOrder};
use rusqlite::Connection;

const MIN_SORTED_MATCH_IDS: u64 = 32_768;

/// Select an optional covering page plan inside the caller's pinned transaction.
/// `no_missing` proves that its complete MATCH count also counts eligible rows:
/// either visibility is All, or this same snapshot has no non-present records.
/// The uncorrelated IN scans MATCH once; only the selected page hydrates Metadata.
pub(super) fn page_sql(
    tx: &Connection,
    request: &SearchRequest,
    no_missing: bool,
    total: u64,
) -> Result<Option<String>> {
    let visibility_only = request.filters
        == (SearchFilters {
            visibility: request.filters.visibility,
            ..Default::default()
        });
    if !no_missing
        || request.query.trim().is_empty()
        || request.order_by != SortOrder::Duration
        || request.folder_id.is_some()
        || !visibility_only
        || request.filters.visibility == FileVisibility::Missing
        || !(MIN_SORTED_MATCH_IDS..=MAX_RANKED_IDS as u64).contains(&total)
    {
        return Ok(None);
    }
    // This useful older index is not a required derived-state feature. Supported
    // read-only snapshots without it must retain the existing complete fallback.
    let has_index: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_index_list('audio_files')
                 WHERE name='idx_duration' AND partial=0)
             AND (SELECT COUNT(*) FROM pragma_index_info('idx_duration'))=1
             AND EXISTS(SELECT 1 FROM pragma_index_info('idx_duration')
                 WHERE name='duration' AND cid>=0)",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if !has_index {
        return Ok(None);
    }
    let direction = if request.descending { "DESC" } else { "ASC" };
    Ok(Some(format!(
        "WITH ranked AS MATERIALIZED (
            SELECT af.id,af.duration AS sort_value
            FROM audio_files af INDEXED BY idx_duration
            WHERE af.id IN (SELECT rowid FROM audio_search WHERE audio_search MATCH ?)
            ORDER BY af.duration {direction},af.id ASC LIMIT ? OFFSET ?
        ) SELECT {AUDIO_SELECT} FROM ranked JOIN audio_files af ON af.id=ranked.id
        ORDER BY ranked.sort_value {direction},af.id ASC"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Error, SqliteProvider, compile_query};
    use audiolib_core::DataProvider;
    use jieba_rs::Jieba;
    use rusqlite::{params, params_from_iter, types::Value};

    fn fixture() -> (tempfile::TempDir, Connection) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sorted.db");
        let provider = SqliteProvider::open(&path).unwrap();
        let conn = Connection::open(path).unwrap();
        for (id, duration) in [
            (i64::MIN, None),
            (-7, Some(-0.0)),
            (1, Some(0.0)),
            (9, Some(2.25)),
            (i64::MAX, Some(2.25)),
        ] {
            conn.execute(
                "INSERT INTO audio_files(id,file_path,filename,duration,missing)
                VALUES(?1,?2,'explosion.wav',?3,0)",
                params![id, format!("root/{id}.wav"), duration],
            )
            .unwrap();
        }
        provider
            .maintain_library(audiolib_core::MaintenanceAction::RebuildSearch)
            .unwrap();
        (directory, conn)
    }

    fn request() -> SearchRequest {
        SearchRequest {
            query: "on".into(),
            order_by: SortOrder::Duration,
            ..Default::default()
        }
    }

    fn values(limit: i64, offset: i64) -> [Value; 3] {
        [
            Value::Text(compile_query(&Jieba::new(), "on").unwrap()),
            Value::Integer(limit),
            Value::Integer(offset),
        ]
    }

    #[test]
    fn duration_plan_scans_covering_index_and_one_uncorrelated_match() {
        let (_directory, conn) = fixture();
        for descending in [false, true] {
            let request = SearchRequest {
                descending,
                ..request()
            };
            let sql = page_sql(&conn, &request, true, MIN_SORTED_MATCH_IDS)
                .unwrap()
                .unwrap();
            let plan = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(params_from_iter(values(2, 1)), |row| {
                    row.get::<_, String>(3)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert!(
                plan.iter()
                    .any(|step| step.contains("USING COVERING INDEX idx_duration")),
                "{plan:?}"
            );
            assert!(
                plan.iter().any(|step| step.contains("LIST SUBQUERY")),
                "{plan:?}"
            );
            assert_eq!(
                plan.iter()
                    .filter(|step| step.contains("SCAN audio_search VIRTUAL TABLE INDEX"))
                    .count(),
                1,
                "{plan:?}"
            );
            assert!(
                !plan
                    .iter()
                    .any(|step| step.contains("CORRELATED") || step.contains("=M")),
                "{plan:?}"
            );
            for (limit, offset) in [(2, 0), (2, 1), (7, 0), (2, 99)] {
                let actual = conn
                    .prepare(&sql)
                    .unwrap()
                    .query_map(params_from_iter(values(limit, offset)), |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                let direction = if descending { "DESC" } else { "ASC" };
                let expected = conn.prepare(&format!("SELECT af.id FROM audio_search CROSS JOIN audio_files af ON af.id=audio_search.rowid
                    WHERE audio_search MATCH ? ORDER BY af.duration {direction},af.id ASC LIMIT ? OFFSET ?")).unwrap()
                    .query_map(params_from_iter(values(limit,offset)),|row| row.get::<_,i64>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
                assert_eq!(actual, expected, "descending={descending},offset={offset}");
            }
        }
    }

    #[test]
    fn optional_duration_plan_preserves_budgets_scope_and_fallbacks() {
        let (_directory, conn) = fixture();
        let request = request();
        assert!(
            page_sql(&conn, &request, true, MIN_SORTED_MATCH_IDS - 1)
                .unwrap()
                .is_none()
        );
        assert!(
            page_sql(&conn, &request, true, MAX_RANKED_IDS as u64 + 1)
                .unwrap()
                .is_none()
        );
        assert!(
            page_sql(&conn, &request, false, MIN_SORTED_MATCH_IDS)
                .unwrap()
                .is_none()
        );
        assert!(
            page_sql(&conn, &request, true, MAX_RANKED_IDS as u64)
                .unwrap()
                .is_some()
        );
        for changed in [
            SearchRequest {
                query: String::new(),
                ..request.clone()
            },
            SearchRequest {
                order_by: SortOrder::Filename,
                ..request.clone()
            },
            SearchRequest {
                folder_id: Some(1),
                ..request.clone()
            },
            SearchRequest {
                filters: SearchFilters {
                    visibility: FileVisibility::Missing,
                    ..Default::default()
                },
                ..request.clone()
            },
            SearchRequest {
                filters: SearchFilters {
                    sample_rate: Some(48000),
                    ..Default::default()
                },
                ..request.clone()
            },
        ] {
            assert!(
                page_sql(&conn, &changed, true, MIN_SORTED_MATCH_IDS)
                    .unwrap()
                    .is_none()
            );
        }
        conn.execute_batch("DROP INDEX idx_duration").unwrap();
        assert!(
            page_sql(&conn, &request, true, MIN_SORTED_MATCH_IDS)
                .unwrap()
                .is_none()
        );
        conn.execute_batch(
            "CREATE INDEX idx_duration ON audio_files(duration) WHERE duration IS NOT NULL",
        )
        .unwrap();
        assert!(
            page_sql(&conn, &request, true, MIN_SORTED_MATCH_IDS)
                .unwrap()
                .is_none()
        );
        conn.execute_batch(
            "DROP INDEX idx_duration; CREATE INDEX idx_duration ON audio_files(sample_rate)",
        )
        .unwrap();
        assert!(
            page_sql(&conn, &request, true, MIN_SORTED_MATCH_IDS)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn duration_membership_query_interrupts_and_the_connection_remains_reusable() {
        let (_directory, conn) = fixture();
        let sql = page_sql(&conn, &request(), true, MIN_SORTED_MATCH_IDS)
            .unwrap()
            .unwrap();
        conn.progress_handler(1, Some(|| true));
        let result = conn
            .prepare(&sql)
            .and_then(|mut statement| {
                statement
                    .query_map(params_from_iter(values(2, 0)), |row| row.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(db_error);
        assert!(matches!(result, Err(Error::Cancelled)));
        conn.progress_handler(0, None::<fn() -> bool>);
        let ids = conn
            .prepare(&sql)
            .unwrap()
            .query_map(params_from_iter(values(2, 0)), |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(ids, vec![i64::MIN, -7]);
    }
}
