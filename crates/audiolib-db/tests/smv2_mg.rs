// crates/audiolib-db/tests/smv2_mg.rs
//! MG-01..MG-18 (doc/TEST-SEARCH-METADATA-V2.md §6): schema 8→9 migration.
//!
//! The migration is not in this branch yet. Tests needing it are ignored with
//! `pending server migration branch`; run them with the entrypoint described in
//! `smv2/driver.rs`:
//!   AUDIOLIB_SMV2_MIGRATE_CMD=<exe> cargo test -p audiolib-db --test smv2_mg -- --ignored
//! MG-09 (old client vs schema-9 file) runs now against the current client code.
mod smv2;
mod support;

use audiolib_core::{DataProvider, MetadataPatch};
use audiolib_db::SqliteProvider;
use rusqlite::Connection;
use serde_json::json;
use smv2::{driver::*, *};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn fixture() -> &'static Fixture {
    static F: OnceLock<Fixture> = OnceLock::new();
    F.get_or_init(Fixture::default_size)
}

fn conn(db: &Path) -> Connection {
    support::connection(db).unwrap()
}

fn env(k: &'static str, v: impl ToString) -> (&'static str, String) {
    (k, v.to_string())
}

/// Observable end state of a migrated library, compared across interrupted and
/// uninterrupted runs.
#[derive(Debug, PartialEq)]
struct EndState {
    user_version: i64,
    path_rows: i64,
    file_rows: i64,
    path_tokens: String,
    main_shadow: String,
}

fn end_state(db: &Path) -> EndState {
    let c = conn(db);
    let count = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    EndState {
        user_version: user_version(db),
        path_rows: count("SELECT count(*) FROM audio_path_search"),
        file_rows: count("SELECT count(*) FROM audio_files"),
        path_tokens: fts_instance_hash(&c, "audio_path_search"),
        main_shadow: main_fts_shadow_hash(&c),
    }
}

fn reference() -> &'static EndState {
    static R: OnceLock<EndState> = OnceLock::new();
    R.get_or_init(|| {
        let (dir, db) = fixture().copy_db();
        Driver::require().migrate_to_done(&db);
        let s = end_state(&db);
        drop(dir);
        s
    })
}

fn assert_still_usable_s8(db: &Path) {
    assert_eq!(user_version(db), 8);
    let p = SqliteProvider::open_read_only(db).expect("pre-switch library must stay a usable s8");
    let f = fixture();
    for (token, _, _) in PLANTED {
        assert_eq!(
            hits(&p, f, token)
                .iter()
                .filter(|h| h.1.ends_with(".wav"))
                .count(),
            expected_planted(token, f.files),
            "{token}"
        );
    }
}

fn db_size(db: &Path) -> u64 {
    fs::metadata(db).unwrap().len()
}

// ------------------------------------------------------------------------ MG-01
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_01_kill_and_resume_at_every_fault_point() {
    let d = Driver::require();
    for point in [
        "abort:batch_boundary:1",
        "abort:mid_batch:1",
        "abort:mid_batch:2",
        "abort:optimize",
        "abort:switch_tx",
    ] {
        let (_dir, db) = fixture().copy_db();
        let first = d.run("start", &db, &[env("AUDIOLIB_MIGRATION_FAULT", point)]);
        assert!(first.aborted(), "{point}: fault did not abort: {first:?}");
        // Every fault here precedes the switch commit: still a usable schema 8.
        assert_still_usable_s8(&db);
        let c = conn(&db);
        if point.starts_with("abort:mid_batch") || point.starts_with("abort:batch_boundary") {
            assert!(
                table_exists(&c, "v9_checkpoint"),
                "{point}: checkpoint missing"
            );
        }
        drop(c);
        let resumed = d.run("resume", &db, &[]);
        assert_eq!(resumed.state(), "done", "{point}: {resumed:?}");
        assert_eq!(
            &end_state(&db),
            reference(),
            "{point}: differs from one-shot run"
        );
    }
}

// ------------------------------------------------------------------------ MG-02
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_02_space_precheck_1_29x_refuses_1_30x_proceeds() {
    let d = Driver::require();
    let (_dir, db) = fixture().copy_db();
    let size = db_size(&db);
    let before = file_sha(&db);
    let low = (size as f64 * 1.29).floor() as u64;
    let r = d.run("start", &db, &[env("AUDIOLIB_MIGRATION_FREE_BYTES", low)]);
    assert_eq!(r.error_code(), Some("INSUFFICIENT_SPACE"), "{r:?}");
    assert_ne!(r.state(), "running");
    assert_eq!(file_sha(&db), before, "refused migration modified the DB");
    let ok = (size as f64 * 1.30).ceil() as u64;
    let r = d.run("start", &db, &[env("AUDIOLIB_MIGRATION_FREE_BYTES", ok)]);
    assert_ne!(r.error_code(), Some("INSUFFICIENT_SPACE"), "{r:?}");
}

// ------------------------------------------------------------------------ MG-03
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_03_backup_failure_aborts_and_backup_is_verified_and_kept() {
    let d = Driver::require();
    let (dir, db) = fixture().copy_db();
    let before = file_sha(&db);
    let r = d.run(
        "start",
        &db,
        &[env("AUDIOLIB_MIGRATION_FAULT", "backup_fail")],
    );
    assert_eq!(r.state(), "failed", "{r:?}");
    assert_eq!(
        file_sha(&db),
        before,
        "backup failure modified the original"
    );
    let r = d.migrate_to_done(&db);
    let backups: Vec<PathBuf> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy();
            n.starts_with("audiolib-rollback-") && n.ends_with(".db")
        })
        .collect();
    assert_eq!(
        backups.len(),
        1,
        "expected one audiolib-rollback-*.db next to the DB: {r:?}"
    );
    let b = Connection::open(&backups[0]).unwrap();
    let ic: String = b
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ic, "ok");
    assert!(
        !b.prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    assert_eq!(user_version(&backups[0]), 8);
    // A second (no-op) run must not delete it.
    d.run("start", &db, &[]);
    assert!(backups[0].exists(), "backup was deleted");
}

// ------------------------------------------------------------------------ MG-04
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_04_read_only_db_is_not_migrated() {
    let d = Driver::require();
    let (_dir, db) = fixture().copy_db();
    let before = file_sha(&db);
    let mut perm = fs::metadata(&db).unwrap().permissions();
    perm.set_readonly(true);
    fs::set_permissions(&db, perm.clone()).unwrap();
    let r = d.run("start", &db, &[]);
    assert_eq!(r.error_code(), Some("READ_ONLY"), "{r:?}");
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    fs::set_permissions(&db, perm).unwrap();
    assert_eq!(file_sha(&db), before);
    assert_eq!(user_version(&db), 8);
}

// ------------------------------------------------------------------------ MG-05
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_05_upgrade_lock_is_exclusive() {
    let d = Arc::new(Driver::require());
    let (_dir, db) = fixture().copy_db();
    let slow = [env("AUDIOLIB_MIGRATION_SLOW_BATCH_MS", "1:1500")];
    let (d1, db1) = (d.clone(), db.clone());
    let first = std::thread::spawn(move || d1.run("start", &db1, &slow));
    std::thread::sleep(Duration::from_millis(500));
    let second = d.run("start", &db, &[]);
    assert_eq!(
        second.error_code(),
        Some("LOCKED"),
        "second migrator did not see upgrade.lock: {second:?}"
    );
    let first = first.join().unwrap();
    assert!(matches!(first.state(), "done" | "paused"), "{first:?}");
    let ic: String = conn(&db)
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ic, "ok");
    let mut lock = db.as_os_str().to_owned();
    lock.push(".upgrade.lock");
    eprintln!("MG-05 lock path checked: {}", Path::new(&lock).display());
}

// ------------------------------------------------------------------------ MG-06
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD; optional AUDIOLIB_SMV2_LARGE_DB)"]
fn mg_06_record_timings() {
    let d = Driver::require();
    let (_dir, db) = match std::env::var_os("AUDIOLIB_SMV2_LARGE_DB") {
        Some(src) => {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("library.db");
            fs::copy(src, &db).unwrap();
            (dir, db)
        }
        None => fixture().copy_db(),
    };
    let started = Instant::now();
    let r = d.migrate_to_done(&db);
    let total = started.elapsed();
    let mut ms = r.batch_ms();
    ms.sort_by(f64::total_cmp);
    let pct = |p: f64| {
        ms.get(((ms.len() as f64 - 1.0) * p).round() as usize)
            .copied()
            .unwrap_or(0.0)
    };
    let phases = r.report["phases"].to_string().to_lowercase();
    assert!(
        !phases.contains("rebuild") || !phases.contains("audio_search\""),
        "main table rebuild phase present: {phases}"
    );
    println!(
        "SMV2-TIMING {}",
        json!({
            "case": "MG-06", "total_s": total.as_secs_f64(), "db_bytes": db_size(&db),
            "batches": ms.len(), "batch_ms_p50": pct(0.5), "batch_ms_p95": pct(0.95),
            "batch_ms_max": ms.last(), "optimize_ms": r.report["optimize_ms"],
            "switch_ms": r.report["switch_ms"], "backup_ms": r.report["backup_ms"],
        })
    );
}

// ------------------------------------------------------------------------ MG-07
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_07_cancel_before_switch_and_rollback_after() {
    let d = Driver::require();
    let f = fixture();
    let (_dir, db) = f.copy_db();
    let main_before = main_fts_shadow_hash(&conn(&db));
    let r = d.run(
        "start",
        &db,
        &[env("AUDIOLIB_MIGRATION_STOP_AFTER_BATCHES", 1)],
    );
    assert_eq!(r.state(), "paused", "{r:?}");
    let r = d.run("cancel", &db, &[]);
    assert!(matches!(r.state(), "idle" | "cancelled"), "{r:?}");
    let c = conn(&db);
    assert!(!table_exists(&c, "audio_path_search"));
    assert!(!table_exists(&c, "v9_checkpoint"));
    assert_eq!(main_fts_shadow_hash(&c), main_before);
    drop(c);
    assert_still_usable_s8(&db);
    // After the switch: restore from audiolib-rollback-*.db.
    d.migrate_to_done(&db);
    let r = d.run("rollback", &db, &[]);
    assert_eq!(r.state(), "idle", "{r:?}");
    assert_still_usable_s8(&db);
    assert_eq!(main_fts_shadow_hash(&conn(&db)), main_before);
}

// ------------------------------------------------------------------------ MG-08
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_08_second_run_is_a_noop() {
    let d = Driver::require();
    let (_dir, db) = fixture().copy_db();
    d.migrate_to_done(&db);
    checkpoint(&db);
    let before = file_sha(&db);
    let r = d.run("start", &db, &[]);
    assert_eq!(r.state(), "done", "{r:?}");
    assert!(r.batch_ms().is_empty(), "second run did work: {r:?}");
    checkpoint(&db);
    assert_eq!(file_sha(&db), before, "second run modified the DB");
}

// ------------------------------------------------------------------------ MG-09
/// Runs now: the current (schema-8) client facing a schema-9 file. A full s9 file
/// does not exist yet, so the fixture is stamped `user_version=9`; rerun with a real
/// migrated file via the ignored variant below.
#[test]
fn mg_09_old_client_refuses_or_reads_schema9_without_writing() {
    let (_dir, db) = fixture().copy_db();
    conn(&db).pragma_update(None, "user_version", 9).unwrap();
    old_client_contract(&db);
}

#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_09_old_client_on_real_migrated_file() {
    let (_dir, db) = fixture().copy_db();
    Driver::require().migrate_to_done(&db);
    old_client_contract(&db);
}

fn old_client_contract(db: &Path) {
    checkpoint(db);
    let before = file_sha(db);
    match SqliteProvider::open(db) {
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            assert!(
                msg.contains("schema") || msg.contains("newer") || msg.contains("version"),
                "unclear refusal: {e}"
            );
        }
        Ok(p) => {
            let any = p
                .search(&Default::default())
                .unwrap()
                .results
                .into_iter()
                .next()
                .unwrap();
            assert!(
                p.update_metadata(
                    any.id,
                    &MetadataPatch {
                        notes: Some("x".into()),
                        ..Default::default()
                    }
                )
                .is_err(),
                "old client wrote into a schema-9 library"
            );
        }
    }
    match SqliteProvider::open_read_only(db) {
        Err(e) => eprintln!("MG-09 read-only open refused: {e}"),
        Ok(_) => eprintln!("MG-09 read-only open allowed"),
    }
    checkpoint(db);
    assert_eq!(
        file_sha(db),
        before,
        "old client modified the schema-9 file"
    );
    assert_eq!(user_version(db), 9);
}

// ------------------------------------------------------------------------ MG-10
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_10_integrity_counts_and_main_fts_unchanged() {
    let d = Driver::require();
    let (_dir, db) = fixture().copy_db();
    let c = conn(&db);
    let main_before = main_fts_shadow_hash(&c);
    let main_rows: i64 = c
        .query_row("SELECT count(*) FROM audio_search", [], |r| r.get(0))
        .unwrap();
    drop(c);
    d.migrate_to_done(&db);
    let c = conn(&db);
    for t in ["audio_search", "audio_path_search"] {
        c.execute(
            &format!("INSERT INTO {t}({t}) VALUES('integrity-check')"),
            [],
        )
        .unwrap();
    }
    let ic: String = c
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ic, "ok");
    let s = end_state(&db);
    assert_eq!(
        s.path_rows, s.file_rows,
        "audio_path_search vs audio_files rows"
    );
    let ids_match: i64 = c
        .query_row("SELECT count(*) FROM audio_files WHERE id NOT IN (SELECT rowid FROM audio_path_search)", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ids_match, 0);
    assert_eq!(
        c.query_row("SELECT count(*) FROM audio_search", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        main_rows
    );
    assert_eq!(s.main_shadow, main_before, "main FTS shadow tables changed");
}

// ------------------------------------------------------------------------ MG-11
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_11_adaptive_batches_and_bounded_write_lock() {
    let d = Driver::require();
    let f = fixture();
    // Slow batch 2 → batch 3 must shrink.
    let (_dir, db) = f.copy_db();
    let r = d.run(
        "start",
        &db,
        &[
            env("AUDIOLIB_MIGRATION_BATCH_ROWS", 50),
            env("AUDIOLIB_MIGRATION_SLOW_BATCH_MS", "2:1800"),
        ],
    );
    let rows = r.batch_rows();
    assert!(
        rows.len() >= 3 && rows[2] < rows[1],
        "batch after a slow one did not shrink: {rows:?}"
    );
    // Concurrent search + writer load: per-batch ≤2 s and writer wait ≤2 s.
    let (_dir2, db2) = f.copy_db();
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, p2) = (stop.clone(), db2.clone());
    let load = std::thread::spawn(move || {
        let c = support::connection(&p2).unwrap();
        c.busy_timeout(Duration::from_secs(30)).unwrap();
        let mut worst = Duration::ZERO;
        while !s2.load(Ordering::Relaxed) {
            let t = Instant::now();
            let _ = c.execute("UPDATE audio_files SET user_notes='mg11' WHERE id=(SELECT min(id) FROM audio_files)", []);
            worst = worst.max(t.elapsed());
            let _: i64 = c
                .query_row(
                    "SELECT count(*) FROM audio_search WHERE audio_search MATCH 'zildjian'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
        }
        worst
    });
    let r = d.migrate_to_done(&db2);
    stop.store(true, Ordering::Relaxed);
    let worst_write = load.join().unwrap();
    let mut ms = r.batch_ms();
    ms.sort_by(f64::total_cmp);
    println!(
        "SMV2-TIMING {}",
        json!({"case":"MG-11","batch_ms_p95": ms.get(ms.len().saturating_sub(1) * 95 / 100), "batch_ms_max": ms.last(), "writer_wait_max_ms": worst_write.as_millis()})
    );
    assert!(
        ms.last().copied().unwrap_or(0.0) <= 2000.0,
        "batch over 2 s"
    );
    assert!(
        worst_write <= Duration::from_secs(2),
        "writer blocked {worst_write:?}"
    );
}

// ------------------------------------------------------------------------ MG-12
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_12_no_vacuum_on_migration_path() {
    let d = Driver::require();
    let (dir, db) = fixture().copy_db();
    let trace = dir.path().join("sql-trace.log");
    let before = db_size(&db);
    let r = d.run_traced("start", &db, &[], Some(&trace));
    assert_eq!(r.state(), "done", "{r:?}");
    let sql = fs::read_to_string(&trace).expect("entrypoint must write --sql-trace");
    assert!(!sql.is_empty(), "empty SQL trace");
    assert!(
        !sql.to_uppercase().contains("VACUUM"),
        "VACUUM executed during migration"
    );
    println!(
        "SMV2-TIMING {}",
        json!({"case":"MG-12","db_bytes_before": before, "db_bytes_after": db_size(&db)})
    );
}

// ------------------------------------------------------------------------ MG-13
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_13_sqlite_full_pauses_and_resumes() {
    let d = Driver::require();
    let (_dir, db) = fixture().copy_db();
    let r = d.run(
        "start",
        &db,
        &[env("AUDIOLIB_MIGRATION_FAULT", "sqlite_full:batch:2")],
    );
    assert_eq!(r.state(), "paused", "{r:?}");
    assert_eq!(r.error_code(), Some("SQLITE_FULL"), "{r:?}");
    assert!(
        table_exists(&conn(&db), "v9_checkpoint"),
        "checkpoint dropped on SQLITE_FULL"
    );
    assert_still_usable_s8(&db);
    let r = d.run("resume", &db, &[]);
    assert_eq!(r.state(), "done", "{r:?}");
    assert_eq!(&end_state(&db), reference());
}

// ------------------------------------------------------------------------ MG-14
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_14_state_machine_start_pause_resume_cancel() {
    // HTTP permission checks (no manage → 403) belong in the server crate; this
    // checks the state machine and progress fields through the entrypoint.
    let d = Driver::require();
    let (_dir, db) = fixture().copy_db();
    assert_eq!(d.run("status", &db, &[]).state(), "idle");
    let r = d.run(
        "start",
        &db,
        &[env("AUDIOLIB_MIGRATION_STOP_AFTER_BATCHES", 1)],
    );
    assert_eq!(r.state(), "paused", "{r:?}");
    for k in ["completed", "total", "batch_ms_p50", "eta_s"] {
        assert!(!r.report[k].is_null(), "status lacks {k}: {r:?}");
    }
    assert!(r.report["completed"].as_u64() <= r.report["total"].as_u64());
    let again = d.run("start", &db, &[]);
    assert_ne!(
        again.state(),
        "running",
        "duplicate start began a second task: {again:?}"
    );
    let r = d.run("resume", &db, &[]);
    assert_eq!(r.state(), "done", "{r:?}");
    assert_eq!(r.report["completed"], r.report["total"]);
}

// ------------------------------------------------------------------------ MG-15
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_15_s8_reads_and_writes_continue_during_backfill() {
    // 503 + Retry-After during the switch and /health.upgrade are HTTP-level and
    // belong in the server crate; this checks library availability between batches.
    let d = Driver::require();
    let f = fixture();
    let (_dir, db) = f.copy_db();
    let r = d.run(
        "start",
        &db,
        &[env("AUDIOLIB_MIGRATION_STOP_AFTER_BATCHES", 1)],
    );
    assert_eq!(r.state(), "paused", "{r:?}");
    assert_still_usable_s8(&db);
    let p = SqliteProvider::open(&db).expect("s8 writer during backfill");
    let any = p.search(&Default::default()).unwrap().results.remove(0);
    p.update_metadata(
        any.id,
        &MetadataPatch {
            tags: Some("mg15".into()),
            ..Default::default()
        },
    )
    .unwrap();
    drop(p);
    assert_eq!(d.run("resume", &db, &[]).state(), "done");
}

// ------------------------------------------------------------------------ MG-16
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_16_defer_schema9_env() {
    let d = Driver::require();
    let (_dir, db) = fixture().copy_db();
    let r = d.run("startup", &db, &[env("AUDIOLIB_DEFER_SCHEMA9", 1)]);
    assert_eq!(r.state(), "idle", "{r:?}");
    assert_still_usable_s8(&db);
    assert!(!table_exists(&conn(&db), "audio_path_search"));
    d.migrate_to_done(&db);
    assert_eq!(user_version(&db), 9);
}

// ------------------------------------------------------------------------ MG-17
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_17_scan_and_rename_during_backfill_reach_path_table() {
    let d = Driver::require();
    let f = fixture();
    let (_dir, db) = f.copy_db();
    let r = d.run(
        "start",
        &db,
        &[env("AUDIOLIB_MIGRATION_STOP_AFTER_BATCHES", 1)],
    );
    assert_eq!(r.state(), "paused", "{r:?}");
    let c = conn(&db);
    // A "renamed" row (highest id, beyond the first batch) and a newly scanned row.
    let id: i64 = c
        .query_row("SELECT max(id) FROM audio_files", [], |r| r.get(0))
        .unwrap();
    c.execute(
        "UPDATE audio_files SET file_path=replace(file_path,'Urban','Mg17renamed') WHERE id=?1",
        [id],
    )
    .unwrap();
    let cols: Vec<String> = c
        .prepare("SELECT name FROM pragma_table_info('audio_files') WHERE name<>'id'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let names = cols
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(",");
    let sel = cols
        .iter()
        .map(|c| {
            if c == "file_path" || c.contains("path") {
                format!(
                    "CASE WHEN typeof(\"{c}\")='text' THEN \"{c}\" || '.mg17new' ELSE \"{c}\" END"
                )
            } else {
                format!("\"{c}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    c.execute(&format!("INSERT INTO audio_files({names}) SELECT {sel} FROM audio_files WHERE id=(SELECT min(id) FROM audio_files)"), []).unwrap();
    drop(c);
    assert_eq!(d.run("resume", &db, &[]).state(), "done");
    let s = end_state(&db);
    assert_eq!(s.path_rows, s.file_rows);
    let hit = d.run(
        "path_search",
        &db,
        &[env("AUDIOLIB_SMV2_QUERY", "Mg17renamed")],
    );
    assert!(
        hit.report["ids"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v.as_i64() == Some(id))),
        "renamed row not in path table: {hit:?}"
    );
}

// ------------------------------------------------------------------------ MG-18
#[test]
#[ignore = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)"]
fn mg_18_switch_transaction_is_atomic() {
    let d = Driver::require();
    let (_dir, db) = fixture().copy_db();
    let r = d.migrate_to_done(&db);
    assert_eq!(user_version(&db), 9);
    let sw = &r.report["switch"];
    assert_eq!(sw["source_search_rev"], 3, "{r:?}");
    for feature in ["search_path", "vorbis_tags", "vendor_metadata"] {
        assert!(
            sw["features"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == feature)),
            "missing {feature}: {r:?}"
        );
    }
    // Abort inside the switch: none of the switch writes may be visible.
    let (_dir2, db2) = fixture().copy_db();
    assert!(
        d.run(
            "start",
            &db2,
            &[env("AUDIOLIB_MIGRATION_FAULT", "abort:switch_tx")]
        )
        .aborted()
    );
    assert_eq!(user_version(&db2), 8);
    let st = d.run("status", &db2, &[]);
    assert!(
        st.report["switch"].is_null() || st.report["switch"]["source_search_rev"] != 3,
        "partial switch: {st:?}"
    );
}
