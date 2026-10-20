// crates/audiolib-db/src/search_facets.rs
//! Exact directory counters from broad retained rankings and a covering scan.
use crate::{
    FileVisibility, FolderCounter, Result, db_error, id_membership::IdMembership, unsigned,
};
use rusqlite::Connection;

const MIN_RANKED_IDS: usize = 262_144;
const BOUNDED_COUNT_SQL: &str = "SELECT COUNT(*) FROM (
    SELECT 1 FROM audio_files INDEXED BY idx_audio_search_facets LIMIT ?1)";

fn count_limit(ranked: usize) -> i64 {
    i64::try_from(ranked)
        .unwrap_or(i64::MAX)
        .saturating_mul(2)
        .saturating_add(1)
}

/// Count complete IDs through bounded JSON arguments and covering folder lookups.
/// Every lookup and the caller's later closure rollup must share this pinned
/// transaction. Each 8192-ID argument stays below 172 KiB even for signed i64s.
pub(super) fn chunked_counts(
    tx: &Connection,
    ids: &[i64],
    visibility: FileVisibility,
    mut check_cancel: impl FnMut() -> Result<()>,
) -> Result<FolderCounter> {
    check_cancel()?;
    let predicate = match visibility {
        FileVisibility::Present => "af.missing=0",
        FileVisibility::Missing => "af.missing=1",
        FileVisibility::All => "1=1",
    };
    let sql = format!(
        "SELECT af.folder_id FROM json_each(?1) matched
         CROSS JOIN audio_files af INDEXED BY idx_audio_search_facets
         ON af.id=matched.value WHERE {predicate}"
    );
    let mut statement = tx.prepare(&sql).map_err(db_error)?;
    let mut counter = FolderCounter::default();
    let mut visited = 0usize;
    for chunk in ids.chunks(8192) {
        check_cancel()?;
        let json = serde_json::to_string(chunk)
            .map_err(|error| crate::Error::Database(error.to_string()))?;
        let mut rows = statement.query([json]).map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            if visited.is_multiple_of(1024) {
                check_cancel()?;
            }
            visited += 1;
            counter.push(row.get(0).map_err(db_error)?);
        }
        check_cancel()?;
    }
    check_cancel()?;
    Ok(counter)
}

/// Count a broad dense ranking without JSON or a point lookup for each match.
/// The caller must verify that all IDs belong to this pinned transaction, and
/// retain its cancellation progress handler throughout the read. None keeps
/// smaller or sparse rankings on the existing chunked SQL path. The extra
/// identity bitmap is bounded to 1 MiB; no fallback HashSet is allocated here.
pub(super) fn ranked_counts(
    tx: &Connection,
    ids: &[i64],
    visibility: FileVisibility,
    check_cancel: impl FnMut() -> Result<()>,
) -> Result<Option<FolderCounter>> {
    ranked_counts_with_minimum(tx, ids, visibility, check_cancel, MIN_RANKED_IDS)
}

fn ranked_counts_with_minimum(
    tx: &Connection,
    ids: &[i64],
    visibility: FileVisibility,
    mut check_cancel: impl FnMut() -> Result<()>,
    minimum: usize,
) -> Result<Option<FolderCounter>> {
    check_cancel()?;
    if ids.len() < minimum {
        return Ok(None);
    }
    // LIMIT prevents SQLite's special full-table count from silently choosing
    // another narrow index. Read only enough of this same covering index to
    // distinguish N<=2*ranked from N>2*ranked, within the pinned snapshot.
    let library_rows = tx
        .query_row(BOUNDED_COUNT_SQL, [count_limit(ids.len())], |row| {
            unsigned(row, 0)
        })
        .map_err(db_error)?;
    check_cancel()?;
    if u128::from(library_rows) > (ids.len() as u128) * 2 {
        return Ok(None);
    }
    let Some(mut membership) = IdMembership::try_dense_for_ranked_ids(ids, &mut check_cancel)?
    else {
        return Ok(None);
    };
    for (index, &id) in ids.iter().enumerate() {
        if index.is_multiple_of(1024) {
            check_cancel()?;
        }
        membership.insert(id);
    }
    check_cancel()?;
    // These exact predicates preserve legacy NULL/nonbinary missing values:
    // only zero is present, only one is missing, and All includes every value.
    let sql = match visibility {
        FileVisibility::Present => {
            "SELECT id,folder_id FROM audio_files INDEXED BY idx_audio_search_facets
             WHERE missing=0"
        }
        FileVisibility::Missing => {
            "SELECT id,folder_id FROM audio_files INDEXED BY idx_audio_search_facets
             WHERE missing=1"
        }
        FileVisibility::All => {
            "SELECT id,folder_id FROM audio_files INDEXED BY idx_audio_search_facets"
        }
    };
    let mut statement = tx.prepare(sql).map_err(db_error)?;
    let mut rows = statement.query([]).map_err(db_error)?;
    let mut counter = FolderCounter::default();
    let mut visited = 0usize;
    while let Some(row) = rows.next().map_err(db_error)? {
        // Include nonmatching candidates in this cadence: a dense ID domain
        // does not imply that every scanned file belongs to the ranking.
        if visited.is_multiple_of(1024) {
            check_cancel()?;
        }
        visited += 1;
        if membership.contains(&row.get(0).map_err(db_error)?) {
            counter.push(row.get(1).map_err(db_error)?);
        }
    }
    check_cancel()?;
    Ok(Some(counter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use rusqlite::params;
    use std::cell::Cell;

    fn fixture(extreme: bool) -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE audio_files(id INTEGER PRIMARY KEY,missing INTEGER,folder_id INTEGER);
             CREATE INDEX idx_audio_search_facets ON audio_files(id,missing,folder_id);",
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        for index in 0..4096_i64 {
            let id = if !extreme {
                index - 2048
            } else if index < 2048 {
                i64::MIN + index
            } else {
                i64::MAX - (4095 - index)
            };
            let missing = match index % 5 {
                0 => None,
                1 => Some(-1),
                2 => Some(0),
                3 => Some(1),
                _ => Some(2),
            };
            let folder = (index % 7 != 0).then_some(index % 11 - 5);
            tx.execute(
                "INSERT INTO audio_files VALUES(?1,?2,?3)",
                params![id, missing, folder],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        conn
    }

    fn ranked_ids(conn: &Connection) -> Vec<i64> {
        conn.prepare("SELECT id FROM audio_files ORDER BY id DESC")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .enumerate()
            .filter(|(index, _)| index % 3 != 0)
            .map(|(_, id)| id.unwrap())
            .collect()
    }

    fn oracle(conn: &Connection, ids: &[i64], visibility: FileVisibility) -> Vec<(i64, u64)> {
        let predicate = match visibility {
            FileVisibility::Present => "af.missing=0",
            FileVisibility::Missing => "af.missing=1",
            FileVisibility::All => "1=1",
        };
        let sql = format!(
            "SELECT af.folder_id FROM json_each(?1) matched
             CROSS JOIN audio_files af INDEXED BY idx_audio_search_facets
             ON af.id=matched.value WHERE {predicate}"
        );
        let mut statement = conn.prepare(&sql).unwrap();
        let mut rows = statement
            .query([serde_json::to_string(ids).unwrap()])
            .unwrap();
        let mut counter = FolderCounter::default();
        while let Some(row) = rows.next().unwrap() {
            counter.push(row.get(0).unwrap());
        }
        counter.into_counts()
    }

    #[test]
    fn bounded_count_uses_the_facet_index_and_saturates_its_signed_limit() {
        let conn = fixture(false);
        conn.execute_batch("CREATE INDEX idx_duration ON audio_files(missing)")
            .unwrap();
        let plan = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {BOUNDED_COUNT_SQL}"))
            .unwrap()
            .query_map([count_limit(100)], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|detail| detail.contains("idx_audio_search_facets"))
        );
        assert!(plan.iter().all(|detail| !detail.contains("idx_duration")));
        let count: i64 = conn
            .query_row(BOUNDED_COUNT_SQL, [count_limit(100)], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 201);
        assert_eq!(count_limit(0), 1);
        assert_eq!(count_limit(2048), 4097);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(count_limit(usize::MAX), i64::MAX);
    }

    #[test]
    fn chunked_covering_counts_preserve_signed_ids_null_folders_and_visibility() {
        for extreme in [false, true] {
            let conn = fixture(extreme);
            // Repeat the existing SQL input to cross a chunk boundary without
            // needing a large fixture; each occurrence retains its old meaning.
            let mut ids = ranked_ids(&conn).repeat(4);
            ids.extend([i64::MIN, 0, i64::MAX]);
            for visibility in [
                FileVisibility::Present,
                FileVisibility::Missing,
                FileVisibility::All,
            ] {
                assert_eq!(
                    chunked_counts(&conn, &ids, visibility, || Ok(()))
                        .unwrap()
                        .into_counts(),
                    oracle(&conn, &ids, visibility)
                );
            }
        }
    }

    #[test]
    fn chunked_lookup_observes_cancellation_and_leaves_later_reads_usable() {
        let conn = fixture(false);
        let ids = ranked_ids(&conn);
        for stop_after in [1, 3, 4] {
            let calls = Cell::new(0);
            let result = chunked_counts(&conn, &ids, FileVisibility::All, || {
                calls.set(calls.get() + 1);
                if calls.get() == stop_after {
                    Err(Error::Cancelled)
                } else {
                    Ok(())
                }
            });
            assert!(matches!(result, Err(Error::Cancelled)));
            assert_eq!(calls.get(), stop_after);
        }
        assert_eq!(
            chunked_counts(&conn, &ids, FileVisibility::All, || Ok(()))
                .unwrap()
                .into_counts(),
            oracle(&conn, &ids, FileVisibility::All)
        );
    }

    #[test]
    fn dense_and_extreme_two_domain_counts_match_existing_sql_for_every_visibility() {
        for extreme in [false, true] {
            let mut conn = fixture(extreme);
            let tx = conn.transaction().unwrap();
            let ids = ranked_ids(&tx);
            // Exactly half the library must qualify at the conservative boundary.
            for ids in [&ids[..], &ids[..2048]] {
                for visibility in [
                    FileVisibility::Present,
                    FileVisibility::Missing,
                    FileVisibility::All,
                ] {
                    let counts = ranked_counts_with_minimum(&tx, ids, visibility, || Ok(()), 0)
                        .unwrap()
                        .unwrap()
                        .into_counts();
                    assert_eq!(counts, oracle(&tx, ids, visibility));
                }
            }
        }
    }

    #[test]
    fn small_selective_and_sparse_rankings_request_the_existing_path() {
        let conn = fixture(false);
        let ids = ranked_ids(&conn);
        assert!(
            ranked_counts(&conn, &ids, FileVisibility::All, || Ok(()))
                .unwrap()
                .is_none()
        );
        assert!(
            ranked_counts_with_minimum(&conn, &ids[..100], FileVisibility::All, || Ok(()), 0,)
                .unwrap()
                .is_none()
        );
        let mut sparse = ids;
        sparse[0] = i64::MIN;
        sparse[1] = i64::MAX;
        assert!(
            ranked_counts_with_minimum(&conn, &sparse, FileVisibility::All, || Ok(()), 0)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cancellation_covers_discovery_insertion_and_nonmatching_scan_rows() {
        let conn = fixture(false);
        let ids = ranked_ids(&conn);
        // With All, 4096 candidates cross four scan checks even though only
        // 2730 IDs match. Replay cancellation at every observed checkpoint.
        let calls = Cell::new(0);
        ranked_counts_with_minimum(
            &conn,
            &ids,
            FileVisibility::All,
            || {
                calls.set(calls.get() + 1);
                Ok(())
            },
            0,
        )
        .unwrap()
        .unwrap();
        let checkpoints = calls.get();
        assert!(checkpoints >= 4);
        for stop_after in 1..=checkpoints {
            calls.set(0);
            let result = ranked_counts_with_minimum(
                &conn,
                &ids,
                FileVisibility::All,
                || {
                    calls.set(calls.get() + 1);
                    if calls.get() == stop_after {
                        Err(Error::Cancelled)
                    } else {
                        Ok(())
                    }
                },
                0,
            );
            assert!(matches!(result, Err(Error::Cancelled)));
            assert_eq!(calls.get(), stop_after);
        }
        let counts = ranked_counts_with_minimum(&conn, &ids, FileVisibility::All, || Ok(()), 0)
            .unwrap()
            .unwrap()
            .into_counts();
        assert_eq!(counts, oracle(&conn, &ids, FileVisibility::All));
    }

    #[test]
    fn sql_failures_are_errors_instead_of_successful_partial_counts() {
        let conn = fixture(false);
        let ids = ranked_ids(&conn);
        conn.execute_batch("DROP INDEX idx_audio_search_facets")
            .unwrap();
        assert!(matches!(
            ranked_counts_with_minimum(&conn, &ids, FileVisibility::All, || Ok(()), 0),
            Err(Error::Database(_))
        ));
        assert!(matches!(
            chunked_counts(&conn, &ids, FileVisibility::All, || Ok(())),
            Err(Error::Database(_))
        ));
        conn.execute_batch("DROP TABLE audio_files").unwrap();
        assert!(matches!(
            ranked_counts_with_minimum(&conn, &ids, FileVisibility::All, || Ok(()), 0),
            Err(Error::Database(_))
        ));
        assert!(matches!(
            chunked_counts(&conn, &ids, FileVisibility::All, || Ok(())),
            Err(Error::Database(_))
        ));
    }

    #[test]
    fn covering_counts_and_visibility_stay_on_the_pinned_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("facets.db");
        let writer = Connection::open(&database).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
             CREATE TABLE audio_files(id INTEGER PRIMARY KEY,missing INTEGER,folder_id INTEGER);
             CREATE INDEX idx_audio_search_facets ON audio_files(id,missing,folder_id);
             INSERT INTO audio_files VALUES(1,0,10),(2,0,20),(3,0,NULL),(4,1,10);",
            )
            .unwrap();
        let mut reader = Connection::open(&database).unwrap();
        let tx = reader.transaction().unwrap();
        let ids = [2, 1];
        let old = oracle(&tx, &ids, FileVisibility::Present);
        writer
            .execute_batch(
                "BEGIN; UPDATE audio_files SET folder_id=30 WHERE id=1;
             UPDATE audio_files SET missing=1 WHERE id=2; COMMIT;",
            )
            .unwrap();
        let counts = |conn: &Connection| {
            let counts =
                ranked_counts_with_minimum(conn, &ids, FileVisibility::Present, || Ok(()), 0)
                    .unwrap()
                    .unwrap()
                    .into_counts();
            assert_eq!(
                counts,
                chunked_counts(conn, &ids, FileVisibility::Present, || Ok(()))
                    .unwrap()
                    .into_counts()
            );
            counts
        };
        assert_eq!(counts(&tx), old);
        drop(tx);
        let current = counts(&reader);
        assert_ne!(current, old);
        assert_eq!(current, oracle(&reader, &ids, FileVisibility::Present));
    }
}
