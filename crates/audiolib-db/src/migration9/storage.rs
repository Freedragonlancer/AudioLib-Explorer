// crates/audiolib-db/src/migration9/storage.rs
//! SQLite backup, checkpoint, directory change capture and atomic publication.
use super::*;
use crate::management::LibraryLease;

const TRACK: &str = "
CREATE TABLE v9_checkpoint(singleton INTEGER PRIMARY KEY CHECK(singleton=1),phase TEXT NOT NULL,last_id INTEGER,done INTEGER NOT NULL,total INTEGER NOT NULL);
INSERT INTO v9_checkpoint SELECT 1,'path_build',NULL,0,count(*) FROM folders;
CREATE TABLE IF NOT EXISTS v9_control(singleton INTEGER PRIMARY KEY CHECK(singleton=1),cmd TEXT NOT NULL,requested_at TEXT NOT NULL);
DELETE FROM v9_control;
CREATE TABLE v9_dirty_dirs(id INTEGER PRIMARY KEY);
CREATE TRIGGER v9_dir_insert AFTER INSERT ON folders BEGIN INSERT OR IGNORE INTO v9_dirty_dirs VALUES(new.id); END;
CREATE TRIGGER v9_dir_update AFTER UPDATE OF folder_name,parent_id ON folders BEGIN INSERT OR IGNORE INTO v9_dirty_dirs VALUES(new.id); END;
CREATE TRIGGER v9_dir_delete AFTER DELETE ON folders BEGIN INSERT OR IGNORE INTO v9_dirty_dirs VALUES(old.id); END;
";
const DROP_TRACK: &str = "DROP TRIGGER IF EXISTS v9_dir_insert; DROP TRIGGER IF EXISTS v9_dir_update; DROP TRIGGER IF EXISTS v9_dir_delete; DROP TABLE IF EXISTS v9_dirty_dirs; DROP TABLE IF EXISTS v9_checkpoint;";

pub(super) fn prepare(conn: &mut Connection) -> Result<()> {
    conn.execute_batch("PRAGMA journal_mode=WAL;")
        .map_err(db_error)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    // A prior explicit s8 experimental path rebuild is disposable derived state.
    tx.execute_batch("DROP TABLE IF EXISTS audio_dir_search; DROP TABLE IF EXISTS audio_dir_runs;")
        .map_err(db_error)?;
    tx.execute_batch(dir_search::CREATE_DIR_FTS)
        .map_err(db_error)?;
    tx.execute_batch(dir_search::CREATE_RUNS)
        .map_err(db_error)?;
    tx.execute_batch(TRACK).map_err(db_error)?;
    tx.execute(
        "DELETE FROM library_meta WHERE key IN ('search_path_rev','snapshot_format')",
        [],
    )
    .map_err(db_error)?;
    tx.commit().map_err(db_error)
}

fn backup_path(path: &Path, saved: &Saved) -> Result<PathBuf> {
    let name = saved
        .backup
        .as_deref()
        .ok_or_else(|| invalid("Missing migration rollback backup"))?;
    if !name.starts_with("audiolib-rollback-mig9-")
        || !name.ends_with(".db")
        || Path::new(name).components().count() != 1
    {
        return Err(invalid("Invalid migration backup name"));
    }
    Ok(path.parent().unwrap().join(name))
}
pub(super) fn validate_backup(path: &Path, saved: &Saved) -> Result<()> {
    let conn = open(&backup_path(path, saved)?, false)?;
    if version(&conn)? != 8
        || identity(&conn)? != saved.library_id
        || !crate::schema::current_format_ready(&conn)?
    {
        return Err(invalid("Rollback backup identity or format mismatch"));
    }
    check(&conn)
}
fn check(conn: &Connection) -> Result<()> {
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .map_err(db_error)?;
    if check != "ok"
        || conn
            .prepare("PRAGMA foreign_key_check")
            .map_err(db_error)?
            .exists([])
            .map_err(db_error)?
    {
        return Err(invalid("Migration integrity check failed"));
    }
    Ok(())
}
pub(super) fn backup(
    path: &Path,
    source: &Connection,
    saved: &mut Saved,
    cancel: &AtomicBool,
) -> Result<()> {
    #[cfg(feature = "test-hooks")]
    if hooks::fault("backup_fail") {
        return Err(invalid("Injected backup failure"));
    }
    if saved.backup.is_some() {
        return validate_backup(path, saved);
    }
    let file = tempfile::Builder::new()
        .prefix(".mig9-backup-")
        .tempfile_in(path.parent().unwrap())?;
    let mut destination = Connection::open(file.path()).map_err(db_error)?;
    {
        let backup = rusqlite::backup::Backup::new(source, &mut destination).map_err(db_error)?;
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            match backup.step(4096).map_err(db_error)? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::Busy | rusqlite::backup::StepResult::Locked => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => (),
            }
        }
    }
    check(&destination)?;
    if identity(&destination)? != saved.library_id || version(&destination)? != 8 {
        return Err(invalid("Backup source changed"));
    }
    drop(destination);
    file.as_file().sync_all()?;
    let name = format!("audiolib-rollback-mig9-{}.db", uuid::Uuid::new_v4());
    file.persist_noclobber(path.parent().unwrap().join(&name))
        .map_err(|e| Error::Io(e.error))?;
    saved.backup = Some(name);
    save(path, saved)
}

pub(super) fn request_control(path: &Path, action: Action) -> Result<()> {
    // Control rows are writes too: an unrelated restore/maintenance operation
    // must exclude them under the same administration lease as annotations.
    let _administration = AdministrationLease::shared(path)?;
    let conn = open(path, true)?;
    if !exists(&conn, "v9_control")? {
        return Err(Error::Busy(
            "Migration is still preparing; retry control request".into(),
        ));
    }
    conn.execute("INSERT INTO v9_control VALUES(1,?1,strftime('%Y-%m-%dT%H:%M:%fZ','now')) ON CONFLICT(singleton) DO UPDATE SET cmd=excluded.cmd,requested_at=excluded.requested_at", [if action == Action::Cancel { "cancel" } else { "pause" }]).map_err(db_error)?;
    Ok(())
}
pub(super) fn clear_control(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM v9_control", [])
        .map_err(db_error)?;
    Ok(())
}
fn drop_paths(conn: &Connection) -> Result<()> {
    conn.execute_batch(DROP_TRACK).map_err(db_error)?;
    conn.execute_batch("DROP TABLE IF EXISTS audio_dir_search; DROP TABLE IF EXISTS audio_dir_runs; DROP TABLE IF EXISTS v9_control;").map_err(db_error)?;
    // Keep the generation monotonic across rollback/re-migration so live readers
    // cannot reuse a cached directory tree from the earlier publication.
    conn.execute(
        "DELETE FROM library_meta WHERE key IN ('search_path_rev','snapshot_format')",
        [],
    )
    .map_err(db_error)?;
    Ok(())
}
pub(super) fn discard(path: &Path) -> Result<()> {
    let mut conn = open(path, true)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    if version(&tx)? != 8 {
        return Err(invalid("Cannot cancel a published migration; use rollback"));
    }
    drop_paths(&tx)?;
    tx.commit().map_err(db_error)
}
fn put(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute("INSERT INTO library_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value]).map_err(db_error)?;
    Ok(())
}

pub(super) fn publish(
    path: &Path,
    conn: &mut Connection,
    saved: &mut Saved,
    cancel: &AtomicBool,
) -> Result<()> {
    if stop(path, conn, saved, cancel)? {
        return Ok(());
    }
    saved.report.state = State::Switching;
    save(path, saved)?;
    // Fail closed if a scan/maintenance is active. The checkpoint remains resumable.
    let _scan = LibraryLease::acquire(path)?;
    let _administration = AdministrationLease::exclusive(path)?;
    let started = Instant::now();
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    if identity(&tx)? != saved.library_id || version(&tx)? != 8 {
        return Err(invalid("Source changed before schema-9 publication"));
    }
    // Replay all committed directory mutations, including insertions behind the cursor.
    let changed = tx
        .prepare("SELECT id FROM v9_dirty_dirs ORDER BY id")
        .map_err(db_error)?
        .query_map([], |r| r.get::<_, i64>(0))
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    for id in changed {
        dir_search::reindex_segment(&tx, id)?;
    }
    tx.execute("DELETE FROM audio_dir_runs", [])
        .map_err(db_error)?;
    tx.execute_batch(dir_search::INSERT_RUNS)
        .map_err(db_error)?;
    tx.execute(
        "INSERT INTO audio_dir_search(audio_dir_search) VALUES('integrity-check')",
        [],
    )
    .map_err(db_error)?;
    let folders: u64 = tx
        .query_row("SELECT count(*) FROM folders", [], |r| count(r, 0))
        .map_err(db_error)?;
    let indexed: u64 = tx
        .query_row("SELECT count(*) FROM audio_dir_search", [], |r| count(r, 0))
        .map_err(db_error)?;
    let missing = tx.query_row("SELECT EXISTS(SELECT 1 FROM folders WHERE id NOT IN (SELECT rowid FROM audio_dir_search))", [], |r| r.get::<_,bool>(0)).map_err(db_error)?;
    if folders != indexed || missing {
        return Err(invalid("Directory index count mismatch"));
    }
    put(
        &tx,
        "snapshot_format",
        audiolib_core::compatibility::PLAN_B_V9_FORMAT,
    )?;
    put(&tx, "search_path_rev", "1")?;
    put(&tx, "source_search_rev", "3")?;
    let generation = tx
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM library_meta WHERE key='dir_index_gen'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(db_error)?
        .unwrap_or(0)
        .saturating_add(1);
    put(&tx, "dir_index_gen", &generation.to_string())?;
    tx.execute(
        "INSERT INTO audiolib_schema_features VALUES('search_path',1)",
        [],
    )
    .map_err(db_error)?;
    tx.pragma_update(None, "user_version", 9)
        .map_err(db_error)?;
    tx.execute_batch(DROP_TRACK).map_err(db_error)?;
    #[cfg(feature = "test-hooks")]
    {
        if let Some(ms) = hooks::number("AUDIOLIB_MIGRATION_HOLD_SWITCH_MS") {
            std::thread::sleep(Duration::from_millis(ms));
        }
        hooks::abort("abort:switch_tx");
    }
    if cancel.load(Ordering::Acquire) {
        drop(tx);
        saved.report.state = State::Paused;
        save(path, saved)?;
        return Ok(());
    }
    tx.commit().map_err(db_error)?;
    saved
        .report
        .phases
        .insert("switch".into(), started.elapsed().as_secs_f64() * 1000.0);
    saved.report.state = State::Done;
    saved.report.progress = Progress {
        done: folders,
        total: folders,
    };
    saved.report.eta_s = Some(0.0);
    saved.report.switch = Some(markers(conn)?);
    save(path, saved)
}

pub(super) fn rollback(path: &Path, mut saved: Saved) -> Result<Report> {
    let _scan = LibraryLease::acquire(path)?;
    let _admin = AdministrationLease::exclusive(path)?;
    let mut conn = open(path, true)?;
    if version(&conn)? != 9 {
        saved.report.error_code = Some(Failure::NotMigrated);
        return Ok(saved.report);
    }
    validate_backup(path, &saved)?;
    validate_ready(&conn)?;
    // This migration changes derived objects only. Reverse those changes in one
    // transaction instead of overwriting annotations made since the retained backup.
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(db_error)?;
    drop_paths(&tx)?;
    tx.execute(
        "DELETE FROM audiolib_schema_features WHERE feature='search_path'",
        [],
    )
    .map_err(db_error)?;
    put(&tx, "source_search_rev", "2")?;
    tx.pragma_update(None, "user_version", 8)
        .map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    saved.report.state = State::RolledBack;
    saved.report.error_code = None;
    saved.report.switch = Some(markers(&conn)?);
    save(path, &saved)?;
    Ok(saved.report)
}

pub(super) fn validate_ready(conn: &Connection) -> Result<()> {
    if version(conn)? != 9 || !crate::schema::readable_format_ready(conn)? {
        return Err(invalid("Invalid schema-9 library contract"));
    }
    let format: Option<String> = conn
        .query_row(
            "SELECT value FROM library_meta WHERE key='snapshot_format'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    let fts: String = conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='audio_dir_search'",
            [],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    let runs: String = conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='audio_dir_runs'",
            [],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    let normalize = |s: &str| {
        s.chars()
            .filter(|c| !c.is_whitespace() && *c != ';')
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    if format.as_deref() != Some(audiolib_core::compatibility::PLAN_B_V9_FORMAT)
        || !dir_search::ready(conn)?
        || normalize(&fts) != normalize(dir_search::CREATE_DIR_FTS)
        || normalize(&runs) != normalize(dir_search::CREATE_RUNS)
        || exists(conn, "v9_checkpoint")?
    {
        return Err(invalid("Incomplete schema-9 directory index"));
    }
    Ok(())
}
