// crates/audiolib-db/src/folder_rollup_tests.rs
//! Folder rollup work must remain bounded after a formerly tiny library grows.
use rusqlite::{Connection, ErrorCode, functions::FunctionFlags, params};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

type RollupRow = (i64, String, String, Option<i64>, i64);

fn grown_library(leaves: usize) -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE folders(
            id INTEGER PRIMARY KEY,folder_path TEXT NOT NULL UNIQUE,
            folder_name TEXT NOT NULL,parent_id INTEGER REFERENCES folders(id));
         CREATE INDEX idx_folders_parent ON folders(parent_id);
         CREATE TABLE folder_closure(
            ancestor_id INTEGER NOT NULL,descendant_id INTEGER NOT NULL,depth INTEGER NOT NULL,
            PRIMARY KEY(ancestor_id,descendant_id));
         CREATE INDEX idx_closure_descendant ON folder_closure(descendant_id);
         CREATE TABLE input_counts(folder_id INTEGER PRIMARY KEY,matches INTEGER NOT NULL);
         INSERT INTO folders VALUES(1,'root','root',NULL);
         INSERT INTO folder_closure VALUES(1,1,0);
         ANALYZE;",
    )
    .unwrap();
    // The real scan path may grow by thousands of directories without ANALYZE.
    // Retain the one-directory statistics rather than hiding that state in a test.
    let tx = conn.transaction().unwrap();
    {
        let mut folder = tx
            .prepare("INSERT INTO folders VALUES(?1,?2,?3,1)")
            .unwrap();
        let mut closure = tx
            .prepare("INSERT INTO folder_closure VALUES(1,?1,1),(?1,?1,0)")
            .unwrap();
        let mut count = tx
            .prepare("INSERT INTO input_counts VALUES(?1,?2)")
            .unwrap();
        for leaf in 0..leaves {
            let id = i64::try_from(leaf).unwrap() + 2;
            folder
                .execute(params![
                    id,
                    format!("root/{leaf:05}"),
                    format!("leaf-{leaf:05}")
                ])
                .unwrap();
            closure.execute([id]).unwrap();
            count.execute(params![id, leaf_matches(leaf)]).unwrap();
        }
        // Keep the old INNER JOIN's handling of an orphan closure ancestor even
        // when hydration avoids a join that stale statistics could turn into a scan.
        tx.execute("INSERT INTO folder_closure VALUES(9000000,2,1)", [])
            .unwrap();
    }
    tx.commit().unwrap();
    let old_stat: String = conn
        .query_row(
            "SELECT stat FROM sqlite_stat1 WHERE idx='idx_closure_descendant'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_stat, "1 1");
    conn
}

fn leaf_matches(leaf: usize) -> i64 {
    3 + i64::try_from(leaf % 7).unwrap()
}

fn read_rollup(conn: &Connection, direct: &str) -> rusqlite::Result<Vec<RollupRow>> {
    conn.prepare(&super::folder_rollup_sql(direct))?
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect()
}

fn verify_rollup(rows: &[RollupRow], leaves: usize) {
    assert_eq!(rows.len(), leaves + 1);
    let root = rows.iter().find(|row| row.0 == 1).unwrap();
    assert_eq!(root.1, "root");
    assert_eq!(root.2, "root");
    assert_eq!(root.3, None);
    assert_eq!(root.4, (0..leaves).map(leaf_matches).sum::<i64>());
    // leaf names precede root in the same NOCASE order used by production.
    for (leaf, row) in rows[..leaves].iter().enumerate() {
        assert_eq!(row.0, i64::try_from(leaf).unwrap() + 2);
        assert_eq!(row.1, format!("root/{leaf:05}"));
        assert_eq!(row.2, format!("leaf-{leaf:05}"));
        assert_eq!(row.3, Some(1));
        assert_eq!(row.4, leaf_matches(leaf));
    }
    assert_eq!(rows.last().unwrap().0, 1);
}

fn measured_vm_work(leaves: usize) -> usize {
    let conn = grown_library(leaves);
    let plan: Vec<String> = conn
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            super::folder_rollup_sql("SELECT folder_id,matches FROM input_counts")
        ))
        .unwrap()
        .query_map([], |row| row.get(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    eprintln!("folder rollup plan ({leaves} leaves): {plan:?}");
    let ticks = Arc::new(AtomicUsize::new(0));
    let observed = ticks.clone();
    conn.progress_handler(
        100,
        Some(move || {
            observed.fetch_add(1, Ordering::Relaxed);
            false
        }),
    )
    .unwrap();
    let rows = read_rollup(&conn, "SELECT folder_id,matches FROM input_counts").unwrap();
    conn.progress_handler(0, None::<fn() -> bool>).unwrap();
    verify_rollup(&rows, leaves);
    ticks.load(Ordering::Relaxed)
}

#[test]
fn stale_tiny_statistics_do_not_make_folder_rollup_quadratic() {
    let small = measured_vm_work(256);
    let large = measured_vm_work(2048);
    assert!(small > 0);
    // Eight times the input permits sorting overhead, but excludes either old
    // nested full scan (closure/direct or totals/folders), which grows 64 times.
    assert!(
        large <= small * 16,
        "rollup VM callbacks grew from {small} to {large} for 8x directories"
    );
}

#[test]
fn cancellation_inside_rollup_interrupts_and_connection_recovers() {
    let leaves = 1024;
    let conn = grown_library(leaves);
    let visits = Arc::new(AtomicUsize::new(0));
    let direct_visits = visits.clone();
    conn.create_scalar_function(
        "observe_direct_match",
        1,
        FunctionFlags::SQLITE_UTF8,
        move |ctx| {
            direct_visits.fetch_add(1, Ordering::Relaxed);
            ctx.get::<i64>(0)
        },
    )
    .unwrap();
    let after_materialization = Arc::new(AtomicUsize::new(0));
    let progress_visits = visits.clone();
    let progress_after = after_materialization.clone();
    conn.progress_handler(
        100,
        Some(move || {
            // Wait until direct counts are fully materialized, then let the
            // closure aggregation execute another 2,000 VM operations.
            progress_visits.load(Ordering::Relaxed) == leaves
                && progress_after.fetch_add(1, Ordering::Relaxed) >= 20
        }),
    )
    .unwrap();
    let error = read_rollup(
        &conn,
        "SELECT folder_id,observe_direct_match(matches) AS matches FROM input_counts",
    )
    .unwrap_err();
    assert!(matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _) if code.code == ErrorCode::OperationInterrupted
    ));
    assert_eq!(visits.load(Ordering::Relaxed), leaves);
    assert!(after_materialization.load(Ordering::Relaxed) > 20);
    conn.progress_handler(0, None::<fn() -> bool>).unwrap();
    let rows = read_rollup(&conn, "SELECT folder_id,matches FROM input_counts").unwrap();
    verify_rollup(&rows, leaves);
}
