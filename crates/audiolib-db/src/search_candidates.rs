// crates/audiolib-db/src/search_candidates.rs
//! Bounded, snapshot-local query IDs and exact filtered intersections.
use crate::{Result, db_error, id_membership::IdMembership, search_cache::MAX_RANKED_IDS};
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value};
use std::sync::Arc;

const RANKING_POLL_INTERVAL: usize = 16_384;

struct Limits {
    candidates: usize,
    matched: usize,
}

/// Collect the complete MATCH membership in this pinned snapshot without scoring
/// or publishing a ranking. An oversized set discards all IDs for a full fallback.
/// `reuse_complete` may supply only a complete query-only ranking proven to share
/// this snapshot and query. It must never wait for a producer or acquire a blocking
/// cache lock; missing/contended entries leave the local cursor making progress.
pub(super) fn query_ids(
    tx: &Connection,
    query: &str,
    check_cancel: impl FnMut() -> Result<()>,
    reuse_complete: impl FnMut() -> Result<Option<Arc<Vec<i64>>>>,
) -> Result<Option<Arc<Vec<i64>>>> {
    query_ids_with_limit(tx, query, check_cancel, reuse_complete, MAX_RANKED_IDS)
}

fn query_ids_with_limit(
    tx: &Connection,
    query: &str,
    mut check_cancel: impl FnMut() -> Result<()>,
    mut reuse_complete: impl FnMut() -> Result<Option<Arc<Vec<i64>>>>,
    limit: usize,
) -> Result<Option<Arc<Vec<i64>>>> {
    check_cancel()?;
    let mut statement = tx
        .prepare("SELECT rowid FROM audio_search WHERE audio_search MATCH ?1")
        .map_err(db_error)?;
    let mut rows = statement.query([query]).map_err(db_error)?;
    let mut ids = Vec::new();
    let limit = limit.min(MAX_RANKED_IDS);
    while let Some(row) = rows.next().map_err(db_error)? {
        if ids.len().is_multiple_of(1024) {
            check_cancel()?;
        }
        if ids.len().is_multiple_of(RANKING_POLL_INTERVAL)
            && let Some(complete) = reuse_complete()?
            && complete.len() <= limit
        {
            check_cancel()?;
            return Ok(Some(complete));
        }
        if ids.len() == limit {
            return Ok(None);
        }
        ids.push(row.get(0).map_err(db_error)?);
    }
    check_cancel()?;
    Ok(Some(Arc::new(ids)))
}

/// A cold present sample-rate/duration filter can scan this narrow index once
/// instead of hitting a small-ranking heuristic and repeating the full FTS join.
/// The index is optional in supported snapshots; force it only for its exact
/// known definition, otherwise retain the ordinary planner and complete fallback.
pub(super) fn visible_filter_from(tx: &Connection) -> Result<&'static str> {
    let sql: Option<String> = tx
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='index' AND name='idx_audio_visible_filter'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    let exact = sql.is_some_and(|sql| {
        let normalized: String = sql
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .flat_map(char::to_lowercase)
            .collect();
        matches!(
            normalized.as_str(),
            "createindexidx_audio_visible_filteronaudio_files(sample_rate,duration,id)wheremissing=0"
                | "createindexifnotexistsidx_audio_visible_filteronaudio_files(sample_rate,duration,id)wheremissing=0"
        )
    });
    Ok(if exact {
        "audio_files af INDEXED BY idx_audio_visible_filter"
    } else {
        "audio_files af"
    })
}

/// Relevance ranking also intersects numeric filters with complete MATCH IDs.
/// Keep this membership scan on a proven covering index: idx_duration alone
/// otherwise visits wide Metadata rows solely to test missing=0.
pub(super) fn covering_numeric_filter_from(
    tx: &Connection,
    request: &audiolib_core::SearchRequest,
) -> Result<Option<&'static str>> {
    use audiolib_core::{FileVisibility, SearchFilters};
    let filters = &request.filters;
    if request.folder_id.is_some()
        || filters.visibility != FileVisibility::Present
        || (filters.sample_rate.is_none()
            && filters.duration_min.is_none()
            && filters.duration_max.is_none())
        || (SearchFilters {
            sample_rate: None,
            duration_min: None,
            duration_max: None,
            ..filters.clone()
        }) != SearchFilters::default()
    {
        return Ok(None);
    }
    let from = visible_filter_from(tx)?;
    Ok((from == "audio_files af INDEXED BY idx_audio_visible_filter").then_some(from))
}

/// Use a bounded full narrow-filter scan for a cold query, keeping the ordinary
/// small-ranking heuristic unchanged for warm rankings and other filter shapes.
pub(super) fn filtered_ids_full_scan(
    tx: &Connection,
    sql: &str,
    values: &[Value],
    query_ids: &[i64],
    check_cancel: impl FnMut() -> Result<()>,
) -> Result<Option<Vec<i64>>> {
    filtered_ids_with_limits(
        tx,
        sql,
        values,
        Some(query_ids),
        check_cancel,
        Limits {
            candidates: MAX_RANKED_IDS,
            matched: MAX_RANKED_IDS,
        },
    )
}

/// Read a parameterized, unique-primary-key SELECT from the caller's pinned
/// snapshot. The SQL must preserve all requested filters and omit query, folder
/// scope and pagination. A supplied complete ranking must belong to this same
/// snapshot. None discards all partial work and requests the original full path.
/// Only the temporary matched IDs are sorted; the cached BM25 order is untouched.
pub(super) fn filtered_ids(
    tx: &Connection,
    sql: &str,
    values: &[Value],
    ranked: Option<&[i64]>,
    check_cancel: impl FnMut() -> Result<()>,
) -> Result<Option<Vec<i64>>> {
    let candidates = ranked.map_or(MAX_RANKED_IDS, |ids| {
        ids.len()
            .saturating_mul(8)
            .saturating_add(8192)
            .min(MAX_RANKED_IDS)
    });
    filtered_ids_with_limits(
        tx,
        sql,
        values,
        ranked,
        check_cancel,
        Limits {
            candidates,
            matched: MAX_RANKED_IDS,
        },
    )
}

fn filtered_ids_with_limits(
    tx: &Connection,
    sql: &str,
    values: &[Value],
    ranked: Option<&[i64]>,
    mut check_cancel: impl FnMut() -> Result<()>,
    limits: Limits,
) -> Result<Option<Vec<i64>>> {
    check_cancel()?;
    if ranked.is_some_and(|ids| ids.len() > MAX_RANKED_IDS) {
        return Ok(None);
    }
    let membership = if let Some(ids) = ranked {
        let mut membership = IdMembership::for_ranked_ids(ids, &mut check_cancel)?;
        for (index, &id) in ids.iter().enumerate() {
            if index.is_multiple_of(1024) {
                check_cancel()?;
            }
            membership.insert(id);
        }
        Some(membership)
    } else {
        None
    };
    check_cancel()?;
    let mut statement = tx.prepare(sql).map_err(db_error)?;
    let mut rows = statement
        .query(params_from_iter(values))
        .map_err(db_error)?;
    let mut matched = Vec::new();
    let mut visited = 0usize;
    let candidate_limit = limits.candidates.min(MAX_RANKED_IDS);
    let matched_limit = limits.matched.min(MAX_RANKED_IDS);
    while let Some(row) = rows.next().map_err(db_error)? {
        // Count every eligible candidate, including IDs absent from the query.
        // A large nonmatching prefix must neither escape the budget nor delay
        // cancellation until the first intersection hit.
        if visited.is_multiple_of(1024) {
            check_cancel()?;
        }
        if visited == candidate_limit {
            return Ok(None);
        }
        visited += 1;
        let id = row.get::<_, i64>(0).map_err(db_error)?;
        if membership.as_ref().is_none_or(|ids| ids.contains(&id)) {
            if matched.len() == matched_limit {
                return Ok(None);
            }
            matched.push(id);
        }
    }
    check_cancel()?;
    matched.sort_unstable();
    check_cancel()?;
    Ok(Some(matched))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use rusqlite::params;
    use std::cell::Cell;

    #[test]
    fn numeric_membership_uses_proven_covering_index_with_complete_visibility() {
        use audiolib_core::{FileVisibility, SearchFilters, SearchRequest};
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE audio_files(id INTEGER PRIMARY KEY,missing INTEGER,sample_rate INTEGER,duration REAL,metadata_json TEXT);
             CREATE INDEX idx_duration ON audio_files(duration);
             CREATE INDEX idx_audio_visible_filter ON audio_files(sample_rate,duration,id) WHERE missing=0;
             INSERT INTO audio_files VALUES(-7,0,48000,1,'wide');
             INSERT INTO audio_files VALUES(2,0,44100,10,'wide');
             INSERT INTO audio_files VALUES(3,1,48000,5,'wide');
             INSERT INTO audio_files VALUES(4,NULL,48000,5,'wide');
             INSERT INTO audio_files VALUES(5,2,48000,5,'wide');
             INSERT INTO audio_files VALUES(6,0,48000,NULL,'wide');",
        ).unwrap();
        let request = SearchRequest {
            query: "metal".into(),
            filters: SearchFilters {
                duration_min: Some(1.0),
                duration_max: Some(10.0),
                ..Default::default()
            },
            ..Default::default()
        };
        let from = covering_numeric_filter_from(&conn, &request)
            .unwrap()
            .unwrap();
        let sql = format!(
            "SELECT af.id FROM {from} WHERE af.missing=0 AND af.duration>=?1 AND af.duration<=?2"
        );
        let ids: Vec<i64> = conn
            .prepare(&sql)
            .unwrap()
            .query_map([1.0, 10.0], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut sorted = ids;
        sorted.sort_unstable();
        assert_eq!(sorted, vec![-7, 2]);
        let plan: Vec<String> = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([1.0, 10.0], |row| row.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|detail| detail.contains("COVERING INDEX idx_audio_visible_filter"))
        );
        for filters in [
            SearchFilters {
                channels: Some(2),
                ..request.filters.clone()
            },
            SearchFilters {
                visibility: FileVisibility::All,
                ..request.filters.clone()
            },
            SearchFilters {
                tagged_only: true,
                ..request.filters.clone()
            },
        ] {
            assert!(
                covering_numeric_filter_from(
                    &conn,
                    &SearchRequest {
                        filters,
                        ..request.clone()
                    }
                )
                .unwrap()
                .is_none()
            );
        }
        assert!(
            covering_numeric_filter_from(
                &conn,
                &SearchRequest {
                    folder_id: Some(1),
                    ..request.clone()
                }
            )
            .unwrap()
            .is_none()
        );
        conn.execute_batch(
            "DROP INDEX idx_audio_visible_filter;
            CREATE INDEX idx_audio_visible_filter ON audio_files(duration,id) WHERE missing=1;",
        )
        .unwrap();
        assert!(
            covering_numeric_filter_from(&conn, &request)
                .unwrap()
                .is_none()
        );
        conn.execute_batch("DROP INDEX idx_audio_visible_filter;")
            .unwrap();
        assert!(
            covering_numeric_filter_from(&conn, &request)
                .unwrap()
                .is_none()
        );
    }

    fn query_fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE VIRTUAL TABLE audio_search USING fts5(text);")
            .unwrap();
        for id in [i64::MIN, -7, i64::MAX] {
            conn.execute(
                "INSERT INTO audio_search(rowid,text) VALUES(?1,'metal')",
                [id],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn query_membership_exact_cap_and_overflow_never_return_partial_ids() {
        let conn = query_fixture();
        let read = |query, limit| query_ids_with_limit(&conn, query, || Ok(()), || Ok(None), limit);
        assert_eq!(
            read("metal", 3).unwrap().unwrap().as_slice(),
            &[i64::MIN, -7, i64::MAX]
        );
        assert!(read("metal", 2).unwrap().is_none());
        assert!(read("metal", 0).unwrap().is_none());
        assert!(read("wood", 0).unwrap().unwrap().is_empty());
        assert!(matches!(read("metal OR", 3), Err(Error::Database(_))));
        conn.execute_batch("DROP TABLE audio_search;").unwrap();
        assert!(matches!(read("metal", 3), Err(Error::Database(_))));
    }

    #[test]
    fn query_scan_cancellation_and_handoff_errors_leave_the_connection_usable() {
        let conn = query_fixture();
        conn.execute_batch(
            "WITH RECURSIVE ids(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM ids WHERE id<2050)
            INSERT INTO audio_search(rowid,text) SELECT id,'metal' FROM ids;",
        )
        .unwrap();
        let calls = Cell::new(0);
        query_ids(
            &conn,
            "metal",
            || {
                calls.set(calls.get() + 1);
                Ok(())
            },
            || Ok(None),
        )
        .unwrap();
        let checkpoints = calls.get();
        for cancel_at in 1..=checkpoints {
            calls.set(0);
            assert!(matches!(
                query_ids(
                    &conn,
                    "metal",
                    || {
                        calls.set(calls.get() + 1);
                        if calls.get() == cancel_at {
                            Err(Error::Cancelled)
                        } else {
                            Ok(())
                        }
                    },
                    || Ok(None)
                ),
                Err(Error::Cancelled)
            ));
        }
        assert!(matches!(
            query_ids(
                &conn,
                "metal",
                || Ok(()),
                || Err(Error::Database("handoff failure".into()))
            ),
            Err(Error::Database(_))
        ));
        assert_eq!(
            query_ids(&conn, "metal", || Ok(()), || Ok(None))
                .unwrap()
                .unwrap()
                .len(),
            2053
        );
    }

    #[test]
    fn handoff_replaces_a_partial_scan_with_one_complete_arc_and_rechecks_cancel() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE audio_search USING fts5(text);
            WITH RECURSIVE ids(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM ids WHERE id<16385)
            INSERT INTO audio_search(rowid,text) SELECT id,'metal' FROM ids;",
        )
        .unwrap();
        let complete = Arc::new((1..=16385).rev().collect::<Vec<i64>>());
        let polls = Cell::new(0);
        let result = query_ids(
            &conn,
            "metal",
            || Ok(()),
            || {
                polls.set(polls.get() + 1);
                Ok((polls.get() == 2).then(|| complete.clone()))
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(polls.get(), 2);
        assert!(Arc::ptr_eq(&result, &complete));
        polls.set(0);
        let result = query_ids(
            &conn,
            "metal",
            || {
                if polls.get() == 2 {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            },
            || {
                polls.set(polls.get() + 1);
                Ok((polls.get() == 2).then(|| complete.clone()))
            },
        );
        assert!(matches!(result, Err(Error::Cancelled)));
        let oversized = Arc::new(vec![1, 2, 3, 4]);
        assert!(
            query_ids_with_limit(&conn, "metal", || Ok(()), || Ok(Some(oversized.clone())), 3)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn old_pinned_query_scan_ignores_a_newer_cached_ranking_without_rewinding_it() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("query-snapshot.db");
        let writer = Connection::open(&database).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
            CREATE VIRTUAL TABLE audio_search USING fts5(text);
            INSERT INTO audio_search(rowid,text) VALUES(1,'metal'),(2,'metal');",
            )
            .unwrap();
        let mut reader = Connection::open(&database).unwrap();
        let tx = reader.transaction().unwrap();
        tx.query_row("SELECT rowid FROM audio_search LIMIT 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        writer
            .execute_batch(
                "BEGIN; DELETE FROM audio_search WHERE rowid=1;
            INSERT INTO audio_search(rowid,text) VALUES(3,'metal'); COMMIT;",
            )
            .unwrap();
        let mut cache = crate::search_cache::RankingCache::default();
        let newer = Arc::new(vec![3, 2]);
        cache.insert(12, "metal".into(), newer.clone());
        let old = query_ids(&tx, "metal", || Ok(()), || Ok(cache.peek(11, "metal")))
            .unwrap()
            .unwrap();
        assert_eq!(old.as_slice(), &[1, 2]);
        assert!(Arc::ptr_eq(&cache.peek(12, "metal").unwrap(), &newer));
        drop(tx);
        assert_eq!(
            query_ids(&reader, "metal", || Ok(()), || Ok(None))
                .unwrap()
                .unwrap()
                .as_slice(),
            &[2, 3]
        );
    }

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE audio_files(id INTEGER PRIMARY KEY,missing INTEGER,
             sample_rate INTEGER,duration REAL,favorite INTEGER,metadata_json TEXT);",
        )
        .unwrap();
        for (id, missing, sample_rate, duration, favorite) in [
            (i64::MIN, Some(0), 48000, 1.0, 1),
            (-8, Some(0), 48000, 2.0, 0),
            (-3, Some(1), 48000, 3.0, 1),
            (0, None, 96000, 4.0, 1),
            (1, Some(2), 48000, 1.0, 1),
            (4, Some(0), 96000, 2.0, 1),
            (9, Some(0), 48000, 5.0, 1),
            (i64::MAX, Some(0), 48000, 2.0, 1),
        ] {
            conn.execute(
                "INSERT INTO audio_files VALUES(?1,?2,?3,?4,?5,'original')",
                params![id, missing, sample_rate, duration, favorite],
            )
            .unwrap();
        }
        conn
    }

    fn oracle(conn: &Connection, sql: &str, values: &[Value], ranked: Option<&[i64]>) -> Vec<i64> {
        let mut values = values.to_vec();
        let predicate = if let Some(ids) = ranked {
            values.push(Value::Text(serde_json::to_string(ids).unwrap()));
            format!(
                "WHERE id IN (SELECT value FROM json_each(?{}))",
                values.len()
            )
        } else {
            String::new()
        };
        conn.prepare(&format!(
            "WITH candidates AS ({sql}) SELECT id FROM candidates {predicate} ORDER BY id"
        ))
        .unwrap()
        .query_map(params_from_iter(&values), |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    }

    #[test]
    fn filtered_intersections_match_sql_for_signed_ids_and_preserve_ranking_order() {
        let conn = fixture();
        let ranked = vec![9, i64::MIN, 4, -3, 1];
        let original = ranked.clone();
        let cases = [
            (
                "SELECT af.id FROM audio_files af WHERE af.missing=0 AND af.sample_rate=?1 AND af.duration>=?2 AND af.duration<=?3",
                vec![Value::Integer(48000), Value::Real(0.0), Value::Real(3.0)],
            ),
            (
                "SELECT af.id FROM audio_files af WHERE af.missing=0 AND af.favorite=1",
                Vec::new(),
            ),
            (
                "SELECT af.id FROM audio_files af WHERE af.missing=1",
                Vec::new(),
            ),
            (
                "SELECT af.id FROM audio_files af WHERE af.favorite=1",
                Vec::new(),
            ),
        ];
        for (sql, values) in cases {
            for ranking in [Some(ranked.as_slice()), None] {
                let result = filtered_ids(&conn, sql, &values, ranking, || Ok(()))
                    .unwrap()
                    .unwrap();
                assert_eq!(result, oracle(&conn, sql, &values, ranking), "{sql}");
                assert_eq!(ranked, original);
            }
        }
        let originals: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM audio_files WHERE metadata_json='original'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(originals, 8);
    }

    #[test]
    fn candidate_and_match_limits_discard_the_complete_partial_result() {
        let conn = fixture();
        let sql = "SELECT id FROM audio_files ORDER BY id";
        let ranked = [i64::MIN, -8];
        assert!(
            filtered_ids_with_limits(
                &conn,
                sql,
                &[],
                Some(&ranked),
                || Ok(()),
                Limits {
                    candidates: 3,
                    matched: 8
                }
            )
            .unwrap()
            .is_none(),
            "nonmatching candidates still consume the scan budget"
        );
        assert!(
            filtered_ids_with_limits(
                &conn,
                sql,
                &[],
                None,
                || Ok(()),
                Limits {
                    candidates: 8,
                    matched: 2
                }
            )
            .unwrap()
            .is_none()
        );
        assert!(
            filtered_ids_with_limits(
                &conn,
                sql,
                &[],
                None,
                || Ok(()),
                Limits {
                    candidates: 7,
                    matched: 8
                }
            )
            .unwrap()
            .is_none()
        );
        let all = filtered_ids_with_limits(
            &conn,
            sql,
            &[],
            None,
            || Ok(()),
            Limits {
                candidates: 8,
                matched: 8,
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(all, oracle(&conn, sql, &[], None));
        assert_eq!(
            filtered_ids_with_limits(
                &conn,
                "SELECT id FROM audio_files WHERE 0",
                &[],
                None,
                || Ok(()),
                Limits {
                    candidates: 0,
                    matched: 0
                }
            )
            .unwrap(),
            Some(Vec::new())
        );
        assert_eq!(
            filtered_ids(&conn, sql, &[], Some(&[]), || Ok(())).unwrap(),
            Some(Vec::new())
        );
    }

    #[test]
    fn cancellation_and_sql_errors_never_become_successful_partial_ids() {
        let conn = fixture();
        let sql = "SELECT id FROM audio_files";
        let ranked = [i64::MIN, 9];
        let calls = Cell::new(0);
        filtered_ids(&conn, sql, &[], Some(&ranked), || {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap();
        let checkpoints = calls.get();
        for cancel_at in 1..=checkpoints {
            calls.set(0);
            let result = filtered_ids(&conn, sql, &[], Some(&ranked), || {
                calls.set(calls.get() + 1);
                if calls.get() == cancel_at {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            });
            assert!(matches!(result, Err(Error::Cancelled)));
        }
        assert!(matches!(
            filtered_ids(&conn, "SELECT id FROM missing_table", &[], None, || Ok(())),
            Err(Error::Database(_))
        ));
        assert!(matches!(
            filtered_ids(
                &conn,
                "SELECT metadata_json FROM audio_files",
                &[],
                None,
                || Ok(())
            ),
            Err(Error::Database(_))
        ));
        assert_eq!(
            filtered_ids(&conn, sql, &[], None, || Ok(()))
                .unwrap()
                .unwrap()
                .len(),
            8
        );
    }

    #[test]
    fn unmatched_candidates_are_cancellable_during_the_stream() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE audio_files(id INTEGER PRIMARY KEY);
            WITH RECURSIVE records(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM records WHERE id<2050)
            INSERT INTO audio_files SELECT id FROM records;").unwrap();
        let calls = Cell::new(0);
        let result = filtered_ids(
            &conn,
            "SELECT id FROM audio_files",
            &[],
            Some(&[999999]),
            || {
                calls.set(calls.get() + 1);
                Ok(())
            },
        )
        .unwrap()
        .unwrap();
        assert!(result.is_empty());
        let checkpoints = calls.get();
        // Discovery and insertion take a fixed handful of checks for one ID;
        // cancellation at each later checkpoint also covers the unmatched scan.
        for cancel_at in 1..=checkpoints {
            calls.set(0);
            let result = filtered_ids(
                &conn,
                "SELECT id FROM audio_files",
                &[],
                Some(&[999999]),
                || {
                    calls.set(calls.get() + 1);
                    if calls.get() == cancel_at {
                        Err(Error::Cancelled)
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(matches!(result, Err(Error::Cancelled)));
        }
    }

    #[test]
    fn filtered_candidates_remain_in_the_callers_pinned_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("filtered-snapshot.db");
        let writer = Connection::open(&database).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
            CREATE TABLE audio_files(id INTEGER PRIMARY KEY,favorite INTEGER);
            INSERT INTO audio_files VALUES(1,1),(2,1),(3,0);",
            )
            .unwrap();
        let mut reader = Connection::open(&database).unwrap();
        let tx = reader.transaction().unwrap();
        tx.query_row("SELECT COUNT(*) FROM audio_files", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        writer
            .execute_batch(
                "BEGIN; UPDATE audio_files SET favorite=0 WHERE id IN (1,2);
            UPDATE audio_files SET favorite=1 WHERE id=3; COMMIT;",
            )
            .unwrap();
        let sql = "SELECT id FROM audio_files WHERE favorite=1";
        assert_eq!(
            filtered_ids(&tx, sql, &[], Some(&[1, 2, 3]), || Ok(())).unwrap(),
            Some(vec![1, 2])
        );
        assert_eq!(
            filtered_ids(&tx, sql, &[], None, || Ok(())).unwrap(),
            Some(vec![1, 2])
        );
        drop(tx);
        assert_eq!(
            filtered_ids(&reader, sql, &[], Some(&[1, 2, 3]), || Ok(())).unwrap(),
            Some(vec![3])
        );
        assert_eq!(
            filtered_ids(&reader, sql, &[], None, || Ok(())).unwrap(),
            Some(vec![3])
        );
    }
}
