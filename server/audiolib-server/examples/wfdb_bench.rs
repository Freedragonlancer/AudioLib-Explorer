//! Synthetic `waveforms.db` benchmark (doc/api/README.md §9.4.1). Not production data.
//! `wfdb_bench <dir> [rows]` runs everything; `wfdb_bench --writer <db>` is the child
//! that is killed with SIGKILL mid-write.
use audiolib_server::wfdb::{self, Row, WaveformsDb};
use rusqlite::Connection;
use std::{path::Path, time::Instant};

fn row(id: i64, salt: i64) -> Row {
    let values: Vec<i16> = (0..256)
        .map(|i| ((id * 31 + i * 7 + salt) % 65535 - 32767) as i16)
        .collect();
    Row {
        file_id: id,
        etag: format!(
            "W/\"wf4-k2-{:016x}-l1i8\"",
            id as u64 * 2654435761 + salt as u64
        ),
        src_mtime: 1_700_000_000 + id,
        src_size: 1_000_000 + id,
        n: 128,
        blob: wfdb::to_blob(&values),
    }
}

fn pct(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[v.len() / 2], v[v.len() * 95 / 100])
}

fn fill(path: &Path, rows: i64, per_txn: i64) -> f64 {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
    let mut conn = Connection::open(path).unwrap();
    wfdb::configure(&conn).unwrap();
    let t = Instant::now();
    let mut id = 1;
    while id <= rows {
        let batch: Vec<Row> = (id..(id + per_txn).min(rows + 1))
            .map(|i| row(i, 0))
            .collect();
        wfdb::upsert_rows(&mut conn, &batch).unwrap();
        id += per_txn;
    }
    conn.pragma_update(None, "wal_checkpoint", "TRUNCATE").ok();
    t.elapsed().as_secs_f64()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args[1] == "--writer" {
        let mut conn = Connection::open(&args[2]).unwrap();
        wfdb::configure(&conn).unwrap();
        let mut salt = 1;
        loop {
            for start in (1..=900_000).step_by(256) {
                let batch: Vec<Row> = (start..(start + 256).min(900_001))
                    .map(|i| row(i, salt))
                    .collect();
                wfdb::upsert_rows(&mut conn, &batch).unwrap();
            }
            salt += 1;
        }
    }
    let dir = Path::new(&args[1]);
    let rows: i64 = args.get(2).map_or(900_000, |r| r.parse().unwrap());
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("waveforms.db");
    let t10k = fill(&path, rows, 10_000);
    let t256 = fill(&path, rows, 256);
    let size = std::fs::metadata(&path).unwrap().len();
    println!(
        "rows={rows} size_bytes={size} full_write_10k_txn_s={t10k:.2} full_write_256_txn_s={t256:.2}"
    );

    // Single UPSERT (own transaction), warm.
    let mut conn = Connection::open(&path).unwrap();
    wfdb::configure(&conn).unwrap();
    let mut normal = Vec::new();
    for k in 0..2000 {
        let t = Instant::now();
        wfdb::upsert_rows(&mut conn, &[row((k * 449) % rows + 1, 7)]).unwrap();
        normal.push(t.elapsed().as_secs_f64() * 1e3);
    }
    conn.pragma_update(None, "synchronous", "FULL").unwrap();
    let mut full = Vec::new();
    for k in 0..500 {
        let t = Instant::now();
        wfdb::upsert_rows(&mut conn, &[row((k * 449) % rows + 1, 8)]).unwrap();
        full.push(t.elapsed().as_secs_f64() * 1e3);
    }
    conn.pragma_update(None, "synchronous", "NORMAL").unwrap();
    let (a, b) = pct(normal);
    let (c, d) = pct(full);
    println!(
        "single_upsert_normal_ms p50={a:.3} p95={b:.3}; synchronous_full p50={c:.3} p95={d:.3}"
    );

    // Read a 200-row page.
    let read_page = |conn: &Connection, start: i64| {
        let mut s = conn
            .prepare_cached("SELECT file_id, etag, n, blob FROM list_waveforms WHERE file_id = ?1")
            .unwrap();
        for id in start..start + 200 {
            let _: Option<Vec<u8>> = s.query_row([id], |r| r.get(3)).ok();
        }
    };
    let reader = Connection::open(&path).unwrap();
    let mut reads = Vec::new();
    for k in 0..500 {
        let start = (k * 7919) % (rows - 200) + 1;
        let t = Instant::now();
        read_page(&reader, start);
        reads.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let (a, b) = pct(reads);
    println!("read_200_page_ms p50={a:.3} p95={b:.3}");

    // Concurrent reads while a writer commits 256-row batches.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let wpath = path.clone();
    let wstop = stop.clone();
    let writer = std::thread::spawn(move || {
        let mut conn = Connection::open(&wpath).unwrap();
        wfdb::configure(&conn).unwrap();
        let mut start = 1;
        let mut n = 0;
        while !wstop.load(std::sync::atomic::Ordering::Relaxed) {
            let batch: Vec<Row> = (start..start + 256).map(|i| row(i, 9)).collect();
            wfdb::upsert_rows(&mut conn, &batch).unwrap();
            n += 1;
            start = (start + 256) % (rows - 256) + 1;
        }
        n
    });
    let mut reads = Vec::new();
    let mut errors = 0;
    for k in 0..1000 {
        let start = (k * 7919) % (rows - 200) + 1;
        let t = Instant::now();
        let ok =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_page(&reader, start)))
                .is_ok();
        if !ok {
            errors += 1
        }
        reads.push(t.elapsed().as_secs_f64() * 1e3);
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let batches = writer.join().unwrap();
    let (a, b) = pct(reads);
    println!(
        "concurrent_read_200_page_ms p50={a:.3} p95={b:.3} errors={errors} writer_batches={batches}"
    );

    // Bundle.
    let db = WaveformsDb::open(dir).unwrap();
    let out = dir.join("bundle.db");
    let _ = std::fs::remove_file(&out);
    let t = Instant::now();
    db.bundle(&out, "bench", 1).unwrap();
    let bt = t.elapsed().as_secs_f64();
    let t = Instant::now();
    wfdb::validate_bundle(&out).unwrap();
    let vt = t.elapsed().as_secs_f64();
    println!(
        "bundle_s={bt:.2} bundle_bytes={} validate_s={vt:.2}",
        std::fs::metadata(&out).unwrap().len()
    );
    std::fs::remove_file(&out).ok();
    drop(db);

    // kill -9 mid-write, then integrity_check.
    let kills: u32 = std::env::var("KILLS")
        .ok()
        .and_then(|k| k.parse().ok())
        .unwrap_or(20);
    let mut ok = 0;
    for k in 0..kills {
        let mut child = std::process::Command::new(&args[0])
            .args(["--writer", path.to_str().unwrap()])
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(
            300 + (k as u64 * 137) % 900,
        ));
        unsafe {
            libc::kill(child.id() as i32, libc::SIGKILL);
        }
        child.wait().unwrap();
        let conn = Connection::open(&path).unwrap();
        let check: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM list_waveforms WHERE length(blob) = 256",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if check == "ok" && count == rows {
            ok += 1
        } else {
            println!("kill {k}: {check} rows={count}")
        }
    }
    println!("kill9 integrity_ok={ok}/{kills}");
}
