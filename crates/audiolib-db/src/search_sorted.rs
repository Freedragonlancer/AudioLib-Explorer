// crates/audiolib-db/src/search_sorted.rs
//! Native alternative ordering over bounded exact MATCH membership sets.
use crate::{
    Result, audio_from_row, audio_projection, db_error, search_cache::MAX_RANKED_IDS,
    search_candidates, search_sql,
};
use audiolib_core::{Error, FileVisibility, SearchFilters, SearchPage, SearchRequest, SortOrder};
use jieba_rs::Jieba;
use rusqlite::{Connection, OptionalExtension, params};
use std::time::Instant;

const MIN_SORTED_MATCH_IDS: u64 = 32_768;

/// Count a filtered filename page from complete narrow membership before touching
/// wide Metadata. All collection, filtering, ordering and hydration must use the
/// caller's pinned transaction and its cancellable SQLite progress handler.
pub(super) fn filtered_filename_page(
    tx: &Connection,
    tokenizer: &Jieba,
    request: &SearchRequest,
    query: &str,
    start: Instant,
    mut check_cancel: impl FnMut() -> Result<()>,
) -> Result<Option<SearchPage>> {
    if request.order_by != SortOrder::Filename
        || request.filters.visibility != FileVisibility::Present
        || request.folder_id.is_some()
        || request.query.trim().is_empty()
        || (SearchFilters {
            sample_rate: None,
            duration_min: None,
            duration_max: None,
            ..request.filters.clone()
        }) != SearchFilters::default()
    {
        return Ok(None);
    }
    check_cancel()?;
    // These are useful optional indexes, not required snapshot identity. Prove
    // their exact known definitions before INDEXED BY or any MATCH collection.
    if !visible_filename_index(tx)? {
        return Ok(None);
    }
    let has_filters = request.filters.sample_rate.is_some()
        || request.filters.duration_min.is_some()
        || request.filters.duration_max.is_some();
    let filter_from = if has_filters {
        let from = search_candidates::visible_filter_from(tx)?;
        if from != "audio_files af INDEXED BY idx_audio_visible_filter" {
            return Ok(None);
        }
        Some(from)
    } else {
        // This proof belongs to the same pinned snapshot as MATCH membership.
        // NULL/nonbinary flags are non-present too; query IDs alone cannot prove
        // visibility. The ordinary planner may use the existing partial index.
        let all_present: bool = tx
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM audio_files WHERE missing IS NOT 0)",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if !all_present {
            return Ok(None);
        }
        None
    };
    let Some(query_ids) = search_candidates::query_ids(tx, query, &mut check_cancel, || Ok(None))?
    else {
        return Ok(None);
    };
    let filtered_ids = if let Some(filter_from) = filter_from.filter(|_| !query_ids.is_empty()) {
        let mut filter_request = request.clone();
        filter_request.query.clear();
        let filter_sql = search_sql(tokenizer, &filter_request, false, false)?;
        let Some(ids) = search_candidates::filtered_ids_full_scan(
            tx,
            &format!(
                "SELECT af.id FROM {filter_from} WHERE {}",
                filter_sql.conditions.join(" AND ")
            ),
            &filter_sql.values,
            &query_ids,
            &mut check_cancel,
        )?
        else {
            return Ok(None);
        };
        Some(ids)
    } else {
        None
    };
    let ids = filtered_ids.as_deref().unwrap_or(&query_ids);
    check_cancel()?;
    let total_count = ids.len() as u64;
    if request.offset as u64 >= total_count {
        return Ok(Some(SearchPage {
            total_count,
            query: request.query.clone(),
            execution_time_ms: start.elapsed().as_secs_f64() * 1000.0,
            ..Default::default()
        }));
    }
    // MAX_RANKED_IDS bounds both vectors. This JSON and SQLite's IN membership
    // structure are additional bounded allocations, not a 16 MiB process cap.
    let json = serde_json::to_string(ids).map_err(|error| Error::Database(error.to_string()))?;
    check_cancel()?;
    let offset = i64::try_from(request.offset)
        .map_err(|_| Error::Validation("Offset is too large".into()))?;
    let mut statement = tx
        .prepare(&filtered_filename_page_sql(
            request.descending,
            &audio_projection(tx)?,
        ))
        .map_err(db_error)?;
    let results = statement
        .query_map(params![json, request.limit as i64, offset], audio_from_row)
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    check_cancel()?;
    Ok(Some(SearchPage {
        returned_count: results.len(),
        results,
        total_count,
        query: request.query.clone(),
        execution_time_ms: start.elapsed().as_secs_f64() * 1000.0,
        cache_hit: false,
    }))
}

fn visible_filename_index(tx: &Connection) -> Result<bool> {
    let sql: Option<String> = tx
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='index' AND name='idx_audio_visible_name'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    Ok(sql.is_some_and(|sql| {
        let normalized = sql
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        matches!(
            normalized.as_str(),
            "createindexidx_audio_visible_nameonaudio_files(filenamecollatenocase,id)wheremissing=0"
                | "createindexifnotexistsidx_audio_visible_nameonaudio_files(filenamecollatenocase,id)wheremissing=0"
        )
    }))
}

fn filtered_filename_page_sql(descending: bool, projection: &str) -> String {
    let direction = if descending { "DESC" } else { "ASC" };
    format!(
        "WITH ranked AS MATERIALIZED (
            SELECT af.id,af.filename COLLATE NOCASE AS sort_value
            FROM audio_files af INDEXED BY idx_audio_visible_name
            WHERE af.missing=0 AND af.id IN (SELECT value FROM json_each(?1))
            ORDER BY sort_value {direction},af.id ASC LIMIT ?2 OFFSET ?3
        ) SELECT {projection} FROM ranked JOIN audio_files af ON af.id=ranked.id
        ORDER BY ranked.sort_value COLLATE NOCASE {direction},af.id ASC"
    )
}

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
    let projection = audio_projection(tx)?;
    Ok(Some(format!(
        "WITH ranked AS MATERIALIZED (
            SELECT af.id,af.duration AS sort_value
            FROM audio_files af INDEXED BY idx_duration
            WHERE af.id IN (SELECT rowid FROM audio_search WHERE audio_search MATCH ?)
            ORDER BY af.duration {direction},af.id ASC LIMIT ? OFFSET ?
        ) SELECT {projection} FROM ranked JOIN audio_files af ON af.id=ranked.id
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
    use std::cell::Cell;

    #[test]
    fn filename_page_plan_reads_the_narrow_name_index_before_hydrating_metadata() {
        let (_directory, conn) = fixture();
        let ids = serde_json::to_string(&[i64::MIN, -7, 1, 9, i64::MAX]).unwrap();
        for descending in [false, true] {
            let plan = conn
                .prepare(&format!(
                    "EXPLAIN QUERY PLAN {}",
                    filtered_filename_page_sql(descending, &audio_projection(&conn).unwrap())
                ))
                .unwrap()
                .query_map(params![ids, 2, 1], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert!(
                plan.iter()
                    .any(|line| line.contains("COVERING INDEX idx_audio_visible_name")),
                "{plan:?}"
            );
            assert!(
                plan.iter().any(|line| line.contains("json_each")),
                "{plan:?}"
            );
            assert!(
                !plan.iter().any(|line| line.contains("audio_search")),
                "page must not repeat MATCH: {plan:?}"
            );
        }
    }

    #[test]
    fn filename_collection_and_page_cancellation_never_return_a_partial_success() {
        let (_directory, conn) = fixture();
        conn.execute("UPDATE audio_files SET sample_rate=48000", [])
            .unwrap();
        let request = SearchRequest {
            order_by: SortOrder::Filename,
            filters: SearchFilters {
                sample_rate: Some(48_000),
                ..Default::default()
            },
            ..request()
        };
        let tokenizer = Jieba::new();
        let query = compile_query(&tokenizer, &request.query).unwrap();
        let calls = Cell::new(0);
        let page =
            filtered_filename_page(&conn, &tokenizer, &request, &query, Instant::now(), || {
                calls.set(calls.get() + 1);
                Ok(())
            })
            .unwrap()
            .unwrap();
        assert_eq!(page.total_count, 5);
        let checkpoints = calls.get();
        for cancel_at in 1..=checkpoints {
            calls.set(0);
            let result =
                filtered_filename_page(&conn, &tokenizer, &request, &query, Instant::now(), || {
                    calls.set(calls.get() + 1);
                    if calls.get() == cancel_at {
                        Err(Error::Cancelled)
                    } else {
                        Ok(())
                    }
                });
            assert!(
                matches!(result, Err(Error::Cancelled)),
                "checkpoint {cancel_at}: {result:?}"
            );
        }
        assert_eq!(
            filtered_filename_page(&conn, &tokenizer, &request, &query, Instant::now(), || Ok(
                ()
            ))
            .unwrap()
            .unwrap()
            .total_count,
            5
        );
        conn.execute_batch("DROP TABLE audio_search_data").unwrap();
        assert!(matches!(
            filtered_filename_page(&conn, &tokenizer, &request, &query, Instant::now(), || Ok(
                ()
            )),
            Err(Error::Database(_))
        ));
    }

    #[test]
    fn unfiltered_filename_visibility_proof_stays_in_its_pinned_snapshot() {
        let (_directory, mut conn) = fixture();
        let writer = Connection::open(conn.path().unwrap()).unwrap();
        let tokenizer = Jieba::new();
        let request = SearchRequest {
            order_by: SortOrder::Filename,
            ..request()
        };
        let query = compile_query(&tokenizer, &request.query).unwrap();
        let tx = conn.transaction().unwrap();
        tx.query_row("SELECT id FROM audio_files LIMIT 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        writer
            .execute("UPDATE audio_files SET missing=1 WHERE id=-7", [])
            .unwrap();
        let page =
            filtered_filename_page(&tx, &tokenizer, &request, &query, Instant::now(), || Ok(()))
                .unwrap()
                .unwrap();
        assert_eq!(page.total_count, 5);
        assert!(page.results.iter().any(|file| file.id == -7));
        drop(tx);
        assert!(
            filtered_filename_page(&conn, &tokenizer, &request, &query, Instant::now(), || Ok(
                ()
            ))
            .unwrap()
            .is_none(),
            "a new snapshot with a missing row must use the complete original path"
        );
    }

    fn fixture() -> (tempfile::TempDir, Connection) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sorted.db");
        let provider = SqliteProvider::open(&path).unwrap();
        let conn = Connection::open(path).unwrap();
        crate::schema::register_tokenizer(&conn, crate::load_tokenizer()).unwrap();
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
        conn.progress_handler(1, Some(|| true)).unwrap();
        let result = conn
            .prepare(&sql)
            .and_then(|mut statement| {
                statement
                    .query_map(params_from_iter(values(2, 0)), |row| row.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(db_error);
        assert!(matches!(result, Err(Error::Cancelled)));
        conn.progress_handler(0, None::<fn() -> bool>).unwrap();
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
