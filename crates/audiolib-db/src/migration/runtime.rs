// crates/audiolib-db/src/migration/runtime.rs
//! Shared startup routing and durable discovery, including the rollback hold.
use super::{MigrationJob, MigrationPhase, apply, job, plan, resume};
use crate::{SnapshotUpgradeProgress, db_error, schema};
use audiolib_core::{Error, Result};
use rusqlite::{Connection, OpenFlags, params};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

const REGISTRY_ID: i32 = 0x414c4d4a;

fn registry_path(source: &Path) -> Result<PathBuf> {
    let source = source.canonicalize()?;
    let mut name = source
        .file_name()
        .ok_or_else(|| super::plan::invalid("Missing library filename"))?
        .to_os_string();
    name.push(".migration-job");
    Ok(source.with_file_name(name))
}

fn registry(source: &Path, create: bool) -> Result<Option<Connection>> {
    let path = registry_path(source)?;
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.is_file() || job::is_link(&metadata) || path.canonicalize()? != path {
                return Err(super::plan::invalid(
                    "Migration registry must be a real file beside the library",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let flags = if create {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let mut conn = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NOFOLLOW)
        .map_err(db_error)?;
    let identity: i32 = conn
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(db_error)?;
    if identity != REGISTRY_ID {
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_schema", [], |row| row.get(0))
            .map_err(db_error)?;
        if !create || identity != 0 || count != 0 {
            return Err(super::plan::invalid(
                "Unrecognized migration registry; refusing to overwrite it",
            ));
        }
        let tx = conn.transaction().map_err(db_error)?;
        tx.execute_batch("CREATE TABLE active_job(singleton INTEGER PRIMARY KEY CHECK(singleton=1),source TEXT NOT NULL,directory TEXT NOT NULL);").map_err(db_error)?;
        tx.pragma_update(None, "application_id", REGISTRY_ID)
            .map_err(db_error)?;
        tx.commit().map_err(db_error)?;
    }
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(db_error)?;
    Ok(Some(conn))
}

pub(super) fn register(job: &MigrationJob) -> Result<()> {
    let conn = registry(&job.plan.source.source, true)?
        .ok_or_else(|| super::plan::invalid("Missing migration registry"))?;
    conn.execute("INSERT INTO active_job VALUES(1,?1,?2) ON CONFLICT(singleton) DO UPDATE SET source=excluded.source,directory=excluded.directory",
        params![job.plan.source.source.to_string_lossy(),job.directory.to_string_lossy()]).map_err(db_error)?;
    Ok(())
}

/// Find the source's owned job without scanning directories or interpreting SemVer.
/// A RolledBack result intentionally inhibits automatic re-upgrade at startup.
pub fn discover(path: &Path) -> Result<Option<MigrationJob>> {
    let source = path.canonicalize()?;
    let conn = job::open_read(&source)?;
    let directory = if super::inspect::table_exists(&conn, "audiolib_storage_migration")? {
        Some(PathBuf::from(
            conn.query_row(
                "SELECT directory FROM audiolib_storage_migration WHERE singleton=1",
                [],
                |row| row.get::<_, String>(0),
            )
            .map_err(db_error)?,
        ))
    } else if let Some(registry) = registry(&source, false)? {
        use rusqlite::OptionalExtension;
        let row: Option<(String, String)> = registry
            .query_row(
                "SELECT source,directory FROM active_job WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(db_error)?;
        match row {
            Some((bound, directory)) if Path::new(&bound) == source => {
                Some(PathBuf::from(directory))
            }
            Some(_) => {
                return Err(super::plan::invalid(
                    "Migration registry belongs to a different library",
                ));
            }
            None => None,
        }
    } else {
        None
    };
    directory
        .map(|directory| {
            let job = job::load(&directory)?;
            if job.plan.source.source != source {
                return Err(super::plan::invalid(
                    "Migration job belongs to a different library",
                ));
            }
            Ok(job)
        })
        .transpose()
}

fn relay(
    event: super::MigrationProgress,
    total: u64,
    progress: &mut dyn FnMut(SnapshotUpgradeProgress),
) {
    let mapped = match event.phase {
        MigrationPhase::Backup => SnapshotUpgradeProgress::BackingUp,
        MigrationPhase::Copying if event.table.as_deref() == Some("audio_files") => {
            SnapshotUpgradeProgress::Copying {
                completed: event.completed,
                total,
            }
        }
        MigrationPhase::Indexing if event.table.as_deref() == Some("audio_files") => {
            SnapshotUpgradeProgress::Indexing {
                completed: event.completed,
                total,
            }
        }
        MigrationPhase::Validating => SnapshotUpgradeProgress::Validating,
        MigrationPhase::Compacting => SnapshotUpgradeProgress::Compacting,
        MigrationPhase::Publishing => SnapshotUpgradeProgress::Publishing,
        MigrationPhase::Checkpoint => SnapshotUpgradeProgress::Checkpoint,
        // Generic job boundaries and other tables carry no audio-row watermark.
        // Do not reset a resumed index's displayed progress to an invented zero.
        _ => return,
    };
    progress(mapped);
}

/// Startup for a writable library. No provider escapes until the target is complete.
pub(crate) fn prepare_writable(
    path: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(SnapshotUpgradeProgress),
) -> Result<()> {
    super::inspect::check_cancel(cancel)?;
    let conn = job::open_read(path)?;
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_error)?;
    let objects: i64 = conn
        .query_row("SELECT COUNT(*) FROM sqlite_schema", [], |row| row.get(0))
        .map_err(db_error)?;
    drop(conn);
    if version == 0 && objects == 0 {
        let _leases = job::Leases::acquire(path)?;
        let mut conn = job::open_rw(path)?;
        schema::register_tokenizer(&conn, crate::load_tokenizer())?;
        return crate::owned_cancellation::run(&mut conn, cancel, schema::create_current);
    }
    if let Some(job) = discover(path)? {
        match job.phase {
            MigrationPhase::RolledBack => return Err(Error::Busy(
                "Library was explicitly rolled back; use migration plan/apply to opt into upgrading again".into())),
            MigrationPhase::Complete => {},
            _ => {
                let total=job.plan.source.business.as_ref().and_then(|digest|digest.tables.iter().find(|table|table.table=="audio_files")).map_or(0,|table|table.rows);
                resume(&job.directory,cancel,&mut |event|{relay(event,total,progress);super::inspect::check_cancel(cancel)})?;
                return Ok(());
            }
        }
    }
    let conn = job::open_read(path)?;
    if !schema::needs_upgrade(&conn)? {
        return Ok(());
    }
    drop(conn);
    if version < 3 {
        return Err(Error::Validation(
            "Legacy libraries must be imported into an owned copy before upgrading".into(),
        ));
    }
    let plan = plan(path, cancel)?;
    let total = plan
        .source
        .business
        .as_ref()
        .and_then(|digest| {
            digest
                .tables
                .iter()
                .find(|table| table.table == "audio_files")
        })
        .map_or(0, |table| table.rows);
    apply(&plan, cancel, &mut |event| {
        relay(event, total, progress);
        super::inspect::check_cancel(cancel)
    })?;
    Ok(())
}
