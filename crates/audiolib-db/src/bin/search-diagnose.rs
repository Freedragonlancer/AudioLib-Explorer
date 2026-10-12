// crates/audiolib-db/src/bin/search-diagnose.rs
//! Read-only SQL plans and stage timings for diagnosing broad-query latency.
use rusqlite::{Connection, OpenFlags};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("Usage: search-diagnose DATABASE")?;
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute_batch(
        "PRAGMA cache_size=-65536; PRAGMA mmap_size=1073741824; PRAGMA temp_store=MEMORY;",
    )?;
    println!("SQLite {}", rusqlite::version());
    for (label, sql) in [
        (
            "count",
            "SELECT COUNT(*) FROM audio_search WHERE audio_search MATCH 'metal'",
        ),
        (
            "rank_ids",
            "SELECT rowid,rank FROM audio_search WHERE audio_search MATCH 'metal' ORDER BY rank LIMIT 200",
        ),
        (
            "rank_join",
            "SELECT af.id,af.filename FROM audio_files af INNER JOIN audio_search ON audio_search.rowid=af.id WHERE af.missing=0 AND audio_search MATCH 'metal' ORDER BY audio_search.rank LIMIT 200",
        ),
        (
            "rank_cte",
            "WITH ranked AS MATERIALIZED (SELECT rowid,rank FROM audio_search WHERE audio_search MATCH 'metal' ORDER BY rank LIMIT 200) SELECT af.id,af.filename FROM ranked INNER JOIN audio_files af ON af.id=ranked.rowid ORDER BY ranked.rank,af.id",
        ),
        (
            "bm25_ids",
            "SELECT rowid,bm25(audio_search) FROM audio_search WHERE audio_search MATCH 'metal' ORDER BY bm25(audio_search),rowid LIMIT 200",
        ),
        (
            "rowids_without_relevance",
            "SELECT rowid FROM audio_search WHERE audio_search MATCH 'metal' LIMIT 200",
        ),
        (
            "folder_counts",
            "SELECT af.folder_id,COUNT(*) FROM audio_files af JOIN audio_search ON audio_search.rowid=af.id WHERE af.missing=0 AND audio_search MATCH 'metal' GROUP BY af.folder_id",
        ),
    ] {
        let mut plan = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
        println!("{label}:");
        for detail in plan.query_map([], |row| row.get::<_, String>(3))? {
            println!("  {}", detail?);
        }
        for pass in 0..3 {
            let start = Instant::now();
            let mut statement = conn.prepare(sql)?;
            let mut rows = statement.query([])?;
            let mut count = 0;
            while rows.next()?.is_some() {
                count += 1;
            }
            println!(
                "  pass {pass}: {} rows, {:.3} ms",
                count,
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
    for pass in 0..3 {
        let start = Instant::now();
        let mut statement = conn.prepare("SELECT af.folder_id FROM audio_search JOIN audio_files af ON af.id=audio_search.rowid WHERE af.missing=0 AND audio_search MATCH 'metal'")?;
        let mut rows = statement.query([])?;
        let mut counts = std::collections::HashMap::<Option<i64>, u64>::new();
        while let Some(row) = rows.next()? {
            *counts.entry(row.get(0)?).or_default() += 1;
        }
        println!(
            "folder_counts_stream pass {pass}: {} folders, {:.3} ms",
            counts.len(),
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
    Ok(())
}
