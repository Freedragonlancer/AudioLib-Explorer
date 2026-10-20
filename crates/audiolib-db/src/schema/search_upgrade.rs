// crates/audiolib-db/src/schema/search_upgrade.rs
//! Resumable, offline search migration. The caller holds scan and administration leases.
use super::*;
use crate::{management, wal};
use rusqlite::params;
use std::path::Path;
use std::sync::atomic::AtomicBool;

const BATCH: i64 = 5_000;
// A future feature revision makes older binaries refuse to reopen a partial migration.
// Only the validated job below may interpret this temporary revision.
const IN_PROGRESS_REVISION: i64 = SEARCH_TEXT_VERSION + 1;
const GUARDED_TABLES: &[&str] = &[
    "audio_files",
    "folders",
    "folder_closure",
    "play_history",
    "scan_sessions",
    "playlists",
    "playlist_items",
    "project_references",
    "waveform_cache",
];

fn write_guards(conn: &Connection, install: bool) -> Result<()> {
    for table in GUARDED_TABLES {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if !exists {
            continue;
        }
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            // Identifiers are exclusively from the static allowlists above.
            let name = format!("audiolib_upgrade_{table}_{operation}");
            conn.execute_batch(&if install {
                format!(
                    "CREATE TRIGGER {name} BEFORE {operation} ON {table}
                    WHEN audiolib_upgrade_writer()!=1 BEGIN
                    SELECT RAISE(ABORT,'Library search upgrade is incomplete'); END;"
                )
            } else {
                format!("DROP TRIGGER IF EXISTS {name};")
            })
            .map_err(db_error)?;
        }
    }
    Ok(())
}

pub(crate) fn pending(conn: &Connection) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='audiolib_search_upgrade' AND type='table')",
        [],
        |row| row.get(0),
    ).map_err(db_error)
}

pub(crate) fn ensure_ready(conn: &Connection) -> Result<()> {
    if pending(conn)? {
        return Err(Error::Busy(
            "Search upgrade is incomplete; reopen the writable library to resume".into(),
        ));
    }
    Ok(())
}

/// Current live libraries use committed batches; new/legacy imports retain atomic setup.
pub(crate) fn migrate_resumable(
    conn: &mut Connection,
    path: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(SnapshotUpgradeProgress) -> Result<()>,
) -> Result<()> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(db_error)?;
    if !pending(conn)? {
        validate_supported_feature_revisions(conn)?;
        if !(1..=SCHEMA_VERSION).contains(&version) || search_text_is_current(conn)? {
            return migrate_with_progress(conn, progress);
        }
        validate_library_identity(conn)?;
        progress(SnapshotUpgradeProgress::BackingUp)?;
        let backup = management::rollback_path(path)?;
        management::snapshot_to_file_cancellable(conn, &backup, cancel)?;
        // Validate the owned rollback copy before changing derived state in the source.
        let mut copy = Connection::open(&backup).map_err(db_error)?;
        crate::owned_cancellation::run(&mut copy, cancel, |copy| {
            let check: String = copy
                .query_row("PRAGMA quick_check", [], |r| r.get(0))
                .map_err(db_error)?;
            if check != "ok" {
                return Err(Error::Database(format!(
                    "Upgrade rollback backup failed validation: {check}"
                )));
            }
            let foreign_keys: i64 = copy
                .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get(0)
                })
                .map_err(db_error)?;
            if foreign_keys != 0 {
                return Err(Error::Database(
                    "Upgrade rollback backup has foreign-key violations".into(),
                ));
            }
            Ok(())
        })?;
        drop(copy);
        tracing::info!(backup = %backup.display(), "Search upgrade rollback backup created");
        let tx = conn.transaction().map_err(db_error)?;
        add_management_schema(&tx)?;
        apply_indexes(&tx)?;
        add_current_features(&tx)?;
        tx.execute_batch(
            "CREATE TABLE audiolib_search_upgrade(
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                protocol INTEGER NOT NULL, target INTEGER NOT NULL, phase TEXT NOT NULL,
                last_id INTEGER, completed INTEGER NOT NULL, total INTEGER NOT NULL,
                rollback TEXT NOT NULL);
             DROP TRIGGER IF EXISTS audio_files_ai;
             DROP TRIGGER IF EXISTS audio_files_au;
             DROP TRIGGER IF EXISTS audio_files_ad;
             DROP TABLE IF EXISTS audio_search;",
        )
        .map_err(db_error)?;
        tx.execute(
            "INSERT INTO audiolib_search_upgrade VALUES(1,1,?1,'tokenizing',NULL,0,(SELECT COUNT(*) FROM audio_files),?2)",
            params![SEARCH_TEXT_VERSION, backup.to_string_lossy()],
        ).map_err(db_error)?;
        set_feature_version(&tx, "tokenization", IN_PROGRESS_REVISION)?;
        write_guards(&tx, true)?;
        tx.commit().map_err(db_error)?;
        wal::truncate(conn)?;
    }
    // Old already-open writers lack this connection-local function, so the durable
    // guards also refuse their writes after an upgrader crashes and releases its lock.
    conn.create_scalar_function(
        "audiolib_upgrade_writer",
        0,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |_| Ok(1i64),
    )
    .map_err(db_error)?;
    loop {
        validate_feature_revisions(conn, IN_PROGRESS_REVISION)?;
        if !(1..=SCHEMA_VERSION).contains(&version) {
            return Err(Error::Database(
                "Unsupported schema for resumable search upgrade".into(),
            ));
        }
        let (protocol, target, phase, last, completed, total): (i64, i64, String, Option<i64>, u64, u64) =
            conn.query_row(
                "SELECT protocol,target,phase,last_id,completed,total FROM audiolib_search_upgrade WHERE singleton=1",
                [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,crate::unsigned(r,4)?,crate::unsigned(r,5)?)),
            ).map_err(db_error)?;
        if protocol != 1
            || target != SEARCH_TEXT_VERSION
            || completed > total
            || !["tokenizing", "indexing", "validating", "checkpoint"].contains(&phase.as_str())
            || feature_version(conn, "tokenization")? != Some(IN_PROGRESS_REVISION)
        {
            return Err(Error::Database(
                "Unsupported or invalid resumable search upgrade".into(),
            ));
        }
        if phase == "checkpoint" {
            progress(SnapshotUpgradeProgress::Checkpoint)?;
            wal::truncate(conn)?;
            let tx = conn.transaction().map_err(db_error)?;
            set_feature_version(&tx, "tokenization", SEARCH_TEXT_VERSION)?;
            write_guards(&tx, false)?;
            tx.execute_batch("DROP TABLE audiolib_search_upgrade;")
                .map_err(db_error)?;
            tx.commit().map_err(db_error)?;
            wal::truncate(conn)?;
            conn.remove_function("audiolib_upgrade_writer", 0)
                .map_err(db_error)?;
            return Ok(());
        }
        if phase == "validating" {
            progress(SnapshotUpgradeProgress::Validating)?;
            // Internal index consistency is bounded by postings, without retokenizing
            // every external-content field a second time. Every row was inserted in
            // the same transaction as its durable cursor under the exclusive lease.
            conn.execute(
                "INSERT INTO audio_search_upgrade(audio_search_upgrade) VALUES('integrity-check')",
                [],
            )
            .map_err(db_error)?;
            let actual: u64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM audio_search_upgrade_docsize",
                    [],
                    |r| crate::unsigned(r, 0),
                )
                .map_err(db_error)?;
            if actual != total {
                return Err(Error::Database("Search upgrade row count mismatch".into()));
            }
            let tx = conn.transaction().map_err(db_error)?;
            // FTS5 renames its shadow tables together, without another rebuild/copy.
            // Older already-open readers never see a partially populated canonical FTS.
            tx.execute_batch("ALTER TABLE audio_search_upgrade RENAME TO audio_search;")
                .map_err(db_error)?;
            let (_, triggers) = FTS_SCHEMA
                .split_once("CREATE TRIGGER")
                .expect("FTS schema has triggers");
            tx.execute_batch(&format!("CREATE TRIGGER{triggers}"))
                .map_err(db_error)?;
            tx.execute_batch(MARK_SOURCE_SEARCH).map_err(db_error)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(db_error)?;
            tx.execute(
                "UPDATE audiolib_search_upgrade SET phase='checkpoint' WHERE singleton=1",
                [],
            )
            .map_err(db_error)?;
            tx.commit().map_err(db_error)?;
            continue;
        }
        let tokenizing = phase == "tokenizing";
        progress(if tokenizing {
            SnapshotUpgradeProgress::Tokenizing { completed, total }
        } else {
            SnapshotUpgradeProgress::Indexing { completed, total }
        })?;
        let lower = if last.is_some() { "id>?1" } else { "id>=?1" };
        let low = last.unwrap_or(i64::MIN);
        let high: Option<i64> = conn.query_row(
            &format!("SELECT MAX(id) FROM (SELECT id FROM audio_files WHERE {lower} ORDER BY id LIMIT ?2)"),
            params![low, BATCH], |r| r.get(0),
        ).map_err(db_error)?;
        let tx = conn.transaction().map_err(db_error)?;
        if let Some(high) = high {
            let changed = if tokenizing {
                tx.execute(
                    &format!("UPDATE audio_files SET
                     filename_tokenized=audiolib_tokenize(filename),
                     user_tags_tokenized=audiolib_tokenize(user_tags),
                     user_notes_tokenized=audiolib_tokenize(user_notes),
                     bwf_description_tokenized=audiolib_tokenize(audiolib_source_text(bwf_description,metadata_json)),
                     ixml_scene_tokenized=audiolib_tokenize(ixml_scene),
                     ixml_take_tokenized=audiolib_tokenize(ixml_take),
                     source_tags_text=audiolib_source_tags_key(metadata_json)
                     WHERE {lower} AND id<=?2"), params![low, high],
                )
            } else {
                tx.execute(
                    &format!("INSERT INTO audio_search_upgrade(rowid,filename,filename_tokenized,user_tags,user_tags_tokenized,
                     bwf_description,bwf_description_tokenized,ixml_scene,ixml_scene_tokenized,
                     ixml_take,ixml_take_tokenized,user_notes,user_notes_tokenized)
                     SELECT id,filename,filename_tokenized,user_tags,user_tags_tokenized,
                     bwf_description,bwf_description_tokenized,ixml_scene,ixml_scene_tokenized,
                     ixml_take,ixml_take_tokenized,user_notes,user_notes_tokenized
                     FROM audio_files WHERE {lower} AND id<=?2 ORDER BY id"),
                    params![low, high],
                )
            }.map_err(db_error)?;
            tx.execute(
                "UPDATE audiolib_search_upgrade SET last_id=?1,completed=completed+?2 WHERE singleton=1",
                params![high, changed as i64],
            ).map_err(db_error)?;
        } else if tokenizing {
            let (table, _) = FTS_SCHEMA
                .split_once("CREATE TRIGGER")
                .expect("FTS schema has triggers");
            tx.execute_batch(&table.replace(
                "CREATE VIRTUAL TABLE audio_search ",
                "CREATE VIRTUAL TABLE audio_search_upgrade ",
            ))
            .map_err(db_error)?;
            tx.execute("UPDATE audiolib_search_upgrade SET phase='indexing',last_id=NULL,completed=0 WHERE singleton=1", []).map_err(db_error)?;
        } else {
            tx.execute(
                "UPDATE audiolib_search_upgrade SET phase='validating' WHERE singleton=1",
                [],
            )
            .map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        // Bound retained WAL between batches. Busy is an explicit retryable failure:
        // never discard a WAL file or publish a partially rebuilt search index.
        wal::truncate(conn)?;
    }
}
