// crates/audiolib-db/src/bin/filter-diagnose.rs
//! Read-only duration-filter plan/scan comparison, separate from end-to-end latency.
use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("Usage: filter-diagnose DATABASE")?;
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute_batch("PRAGMA cache_size=-65536; PRAGMA temp_store=MEMORY;")?;
    conn.pragma_update(
        None,
        "mmap_size",
        audiolib_db::sqlite_reader_mmap_bytes(Path::new(&path)),
    )?;
    let variants = [
        (
            "planner",
            "SELECT af.id FROM audio_files af WHERE af.missing=0 AND af.duration>=?1 AND af.duration<=?2",
        ),
        (
            "covering",
            "SELECT af.id FROM audio_files af INDEXED BY idx_audio_visible_filter WHERE af.missing=0 AND af.duration>=?1 AND af.duration<=?2",
        ),
    ];
    let mut reports = Vec::new();
    let mut expected = None;
    for pass in 0..4 {
        for offset in 0..2 {
            let (name, sql) = variants[(pass + offset) % 2];
            let plan = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
                .query_map(params![1.0, 10.0], |row| row.get::<_, String>(3))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let start = Instant::now();
            let mut ids = conn
                .prepare(sql)?
                .query_map(params![1.0, 10.0], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let scan_ms = start.elapsed().as_secs_f64() * 1000.0;
            ids.sort_unstable();
            let mut digest = Sha256::new();
            for id in &ids {
                digest.update(id.to_le_bytes());
            }
            let hash = format!("{:x}", digest.finalize());
            if let Some(expected) = &expected {
                assert_eq!(expected, &hash);
            } else {
                expected = Some(hash.clone());
            }
            reports.push(serde_json::json!({"pass":pass,"variant":name,"plan":plan,"scan_ms":scan_ms,"rows":ids.len(),"ids_sha256":hash}));
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "sqlite":rusqlite::version(),"scope":"duration membership SQL only; OS cache uncontrolled; ordered IDs verified","records":reports
        }))?
    );
    Ok(())
}
