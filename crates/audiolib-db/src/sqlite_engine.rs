// crates/audiolib-db/src/sqlite_engine.rs
//! Linked-engine identity and the contentless-delete statistics safety boundary.
use audiolib_core::{Error, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// Actual linked library identity; the system sqlite3 executable is irrelevant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SqliteEngineIdentity {
    /// Runtime SQLite version string.
    pub version: String,
    /// Exact source commit and build timestamp reported by SQLite.
    pub source_id: String,
    /// Whether this runtime contains the required contentless-delete baseline.
    pub supports_plan_b: bool,
}

/// Read the actual engine used by this process without opening a persistent file.
pub fn identity() -> Result<SqliteEngineIdentity> {
    let conn = Connection::open_in_memory().map_err(crate::db_error)?;
    Ok(SqliteEngineIdentity {
        version: rusqlite::version().into(),
        source_id: conn
            .query_row("SELECT sqlite_source_id()", [], |row| row.get(0))
            .map_err(crate::db_error)?,
        supports_plan_b: rusqlite::version_number() >= 3_054_001,
    })
}

/// Reject Plan B writers whose native row/column totals may drift after deletion.
pub fn require_plan_b() -> Result<()> {
    if rusqlite::version_number() < 3_054_001 {
        return Err(Error::Database(format!(
            "Search storage B requires SQLite >=3.54.1; linked engine is {}",
            rusqlite::version()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn verify(conn: &Connection) {
        let aggregates = |table: &str| -> Vec<u8> {
            conn.query_row(
                &format!("SELECT block FROM {table}_data WHERE id=1"),
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(aggregates("ordinary"), aggregates("lean"));
        for query in ["metal", "door", "\"metal door\"", "wood", "爆炸"] {
            let results = |table: &str| -> Vec<(i64, u64)> {
                conn.prepare(&format!(
                    "SELECT rowid,bm25({table}) FROM {table} WHERE {table} MATCH ?1 ORDER BY rowid"
                ))
                .unwrap()
                .query_map([query], |row| {
                    Ok((row.get(0)?, row.get::<_, f64>(1)?.to_bits()))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
            };
            assert_eq!(results("ordinary"), results("lean"), "{query}");
        }
    }

    #[test]
    fn linked_engine_contains_the_pinned_fix() {
        require_plan_b().unwrap();
        let engine = identity().unwrap();
        assert!(engine.supports_plan_b);
        assert_eq!(engine.version, "3.54.1");
        assert!(
            engine
                .source_id
                .ends_with("555e07ef33f69a0957d24d8e555b702464a131d6f1607f4c3c1e4551e8e6ca76")
        );
    }

    #[test]
    fn contentless_statistics_and_native_bm25_survive_replacement_delete_and_rollback() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE ordinary USING fts5(a,b,detail=full,columnsize=1);
             CREATE VIRTUAL TABLE lean USING fts5(a,b,content='',contentless_delete=1,
                 detail=full,columnsize=1);",
        )
        .unwrap();
        for table in ["ordinary", "lean"] {
            for (id, a, b) in [
                (i64::MIN, "metal door", "metal"),
                (-5, "wood door", "爆炸"),
                (1, "metal metal", "wood"),
                (i64::MAX, "door", "door metal"),
            ] {
                conn.execute(
                    &format!("INSERT INTO {table}(rowid,a,b) VALUES(?1,?2,?3)"),
                    params![id, a, b],
                )
                .unwrap();
            }
        }
        verify(&conn);
        for round in 0..5 {
            for table in ["ordinary", "lean"] {
                conn.execute(
                    &format!("UPDATE {table} SET a=?1,b=?2 WHERE rowid=1"),
                    params![format!("metal door {round}"), "wood 爆炸"],
                )
                .unwrap();
            }
            verify(&conn);
        }
        conn.execute_batch("BEGIN; DELETE FROM ordinary WHERE rowid=-5; DELETE FROM lean WHERE rowid=-5; ROLLBACK;")
            .unwrap();
        verify(&conn);
        for table in ["ordinary", "lean"] {
            conn.execute(&format!("DELETE FROM {table} WHERE rowid=-5"), [])
                .unwrap();
            conn.execute(
                &format!("INSERT INTO {table}({table}) VALUES('optimize')"),
                [],
            )
            .unwrap();
            conn.execute(
                &format!("INSERT INTO {table}({table}) VALUES('integrity-check')"),
                [],
            )
            .unwrap();
        }
        verify(&conn);
    }
}
