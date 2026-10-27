// crates/audiolib-db/src/schema/current.rs
//! Schema 6 entry points, separate from the retained v1..v5 normalization code.
use super::*;
use crate::{migration, search_storage};
use audiolib_core::compatibility::LibraryFormat;
use rusqlite::{params, params_from_iter, types::Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) fn current_format_ready(conn: &Connection) -> Result<bool> {
    format_ready(conn, false)
}

/// Completed p0 snapshots remain readable; all new library writes require p1.
pub(crate) fn readable_format_ready(conn: &Connection) -> Result<bool> {
    format_ready(conn, true)
}

fn format_ready(conn: &Connection, allow_p0: bool) -> Result<bool> {
    if !format_ready_at_version(conn, allow_p0, SCHEMA_VERSION)? {
        return Ok(false);
    }
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo('audio_files') WHERE name='album' COLLATE NOCASE)",
        [], |row| row.get(0),
    ).map_err(db_error)
}

fn format_ready_at_version(
    conn: &Connection,
    allow_p0: bool,
    expected_version: i64,
) -> Result<bool> {
    ensure_ready(conn)?;
    validate_supported_feature_revisions(conn)?;
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_error)?;
    if version != expected_version || !feature_table_exists(conn)? {
        return Ok(false);
    }
    let features = conn
        .prepare("SELECT feature,version FROM audiolib_schema_features ORDER BY feature")
        .map_err(db_error)?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()
        .map_err(db_error)?;
    let mut expected = LibraryFormat::target().features;
    let p0 = allow_p0 && features.get("play_history_retired") == Some(&1);
    if p0 {
        expected.insert("play_history_retired".into(), 1);
    }
    if features != expected
        || !source_tags_ready(conn)?
        || !source_search_marked(conn)?
        || !search_storage::structure_matches(conn)?
        || search_storage::columns(conn)?.iter().any(|column| {
            column.eq_ignore_ascii_case("play_count")
                || (!p0 && column.eq_ignore_ascii_case("last_played_at"))
        })
    {
        return Ok(false);
    }
    let history: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='play_history' COLLATE NOCASE)",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if history {
        return Ok(false);
    }
    definition::required_match(conn)
}

/// v6 -> v7: add the album column, fill it from the Metadata already stored in the
/// library (no audio file is read), then publish the format with its album index.
pub(crate) fn upgrade_from_previous(
    conn: &mut Connection,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(SnapshotUpgradeProgress) -> Result<()>,
) -> Result<()> {
    progress(SnapshotUpgradeProgress::Preparing)?;
    let tx = conn.transaction().map_err(db_error)?;
    ensure_column(&tx, "audio_files", "album", "TEXT NOT NULL DEFAULT ''")?;
    let total_rows: i64 = tx
        .query_row("SELECT COUNT(*) FROM audio_files", [], |row| row.get(0))
        .map_err(db_error)?;
    let total = u64::try_from(total_rows).unwrap_or(0);
    let mut cursor = i64::MIN;
    let mut completed = 0_u64;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        // All batches share this atomic transaction; cancellation rolls back the DDL too.
        let last: Option<i64> = tx
            .query_row(
                "SELECT MAX(id) FROM
                 (SELECT id FROM audio_files WHERE id >= ?1 ORDER BY id LIMIT 5000)",
                [cursor],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(db_error)?;
        let Some(last) = last else { break };
        let changed = tx
            .execute(
                "UPDATE audio_files SET album=audiolib_album_key(metadata_json)
             WHERE id >= ?1 AND id <= ?2",
                params![cursor, last],
            )
            .map_err(db_error)?;
        completed = completed.saturating_add(changed as u64).min(total);
        progress(SnapshotUpgradeProgress::Tokenizing { completed, total })?;
        let Some(next) = last.checked_add(1) else {
            break;
        };
        cursor = next;
    }
    apply_indexes(&tx)?;
    tx.execute_batch(ALBUM_INDEX).map_err(db_error)?;
    mark_current(&tx)?;
    if cancel.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    tx.commit().map_err(db_error)
}

/// v6 -> v7 without a storage migration: add the album column, fill it from the Metadata
/// already stored in the library, create its index and publish the format. The whole step
/// is one transaction over additive DDL plus one derived UPDATE, so it takes seconds and
/// never touches audio files. Returns false when the library is not a v6 source.
pub(crate) fn fast_upgrade_album(
    conn: &mut Connection,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(SnapshotUpgradeProgress),
) -> Result<bool> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_error)?;
    if pending(conn)? {
        return Ok(false);
    }
    let has_column: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo('audio_files')
             WHERE name='album' COLLATE NOCASE)",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    let upgrade_source = version == SCHEMA_VERSION - 1;
    // Also repair a v7 library whose album column is missing (for example one produced by
    // a storage migration that predates the column); the derived state is rebuilt the same
    // way, so no further migration and no audio file is needed.
    if !upgrade_source && (version != SCHEMA_VERSION || has_column) {
        return Ok(false);
    }
    // Only a complete p1 source can use this additive path. Missing watermarks,
    // damaged FTS structure and p0 retirement require the checked migration route.
    if !format_ready_at_version(conn, false, version)? {
        return Ok(false);
    }
    let extension_triggers: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='trigger' AND tbl_name='audio_files'
         AND name NOT IN ('audio_files_ai','audio_files_au','audio_files_ad','audiolib_audio_id_high_water'))",
        [], |row| row.get(0),
    ).map_err(db_error)?;
    if extension_triggers {
        // The ordinary candidate route verifies their side effects before publication.
        return Ok(false);
    }
    let foreign_keys: i64 = conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .map_err(db_error)?;
    if foreign_keys != 0 {
        return Err(Error::Validation(
            "Library relationships require repair before the album upgrade".into(),
        ));
    }
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(db_error)?;
    if check != "ok" {
        return Err(Error::Database(check));
    }
    upgrade_from_previous(conn, cancel, &mut |event| {
        progress(event);
        if cancel.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    })?;
    Ok(true)
}

pub(crate) fn needs_upgrade(conn: &Connection) -> Result<bool> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_error)?;
    if version > SCHEMA_VERSION {
        return Err(Error::NewerDatabase {
            what: "schema".into(),
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    if version == SCHEMA_VERSION {
        return current_format_ready(conn).map(|ready| !ready);
    }
    if !pending(conn)? {
        validate_supported_feature_revisions(conn)?;
    }
    Ok(true)
}

pub(crate) fn mark_current(conn: &Connection) -> Result<()> {
    search_storage::transaction_required(conn)?;
    let retired: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo('audio_files')
          WHERE name COLLATE NOCASE IN ('play_count','last_played_at'))
          OR EXISTS(SELECT 1 FROM sqlite_schema WHERE name='play_history' COLLATE NOCASE)",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if retired {
        return Err(Error::Validation(
            "Playback retirement must complete in the owned migration before publishing p1".into(),
        ));
    }
    // Repair only explicitly owned derived objects, preserving ID watermark data.
    definition::repair_owned(conn)?;
    for (name, version) in LibraryFormat::target().features {
        conn.execute(
            "INSERT INTO audiolib_schema_features(feature,version) VALUES(?1,?2)
            ON CONFLICT(feature) DO UPDATE SET version=excluded.version",
            params![name, version],
        )
        .map_err(db_error)?;
    }
    conn.execute_batch(MARK_SOURCE_SEARCH).map_err(db_error)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(db_error)
}

/// Build only a genuinely empty database, using the same reviewed schema recipe.
pub(crate) fn create_current(conn: &mut Connection) -> Result<()> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sqlite_schema", [], |row| row.get(0))
        .map_err(db_error)?;
    if count != 0 {
        return Err(Error::Validation(
            "New library creation requires an empty SQLite file".into(),
        ));
    }
    let mut template = Connection::open_in_memory().map_err(db_error)?;
    register_tokenizer(&template, crate::load_tokenizer())?;
    // Build the current business template directly. No playback table/column is
    // created, even transiently; old-source normalization is isolated in legacy.rs.
    let seed = template.transaction().map_err(db_error)?;
    seed.execute_batch(BASE_SCHEMA).map_err(db_error)?;
    for (name, definition) in AUDIO_COLUMNS {
        if !search_storage::is_derived_column(name) {
            ensure_column(&seed, "audio_files", name, definition)?;
        }
    }
    for (name, definition) in FOLDER_COLUMNS {
        ensure_column(&seed, "folders", name, definition)?;
    }
    seed.execute_batch(BASE_INDEXES).map_err(db_error)?;
    add_management_schema(&seed)?;
    apply_indexes(&seed)?;
    seed.execute_batch(ALBUM_INDEX).map_err(db_error)?;
    add_current_features(&seed)?;
    seed.execute_batch(MARK_SOURCE_SEARCH).map_err(db_error)?;
    seed.commit().map_err(db_error)?;
    let blueprint = search_storage::blueprint_with(&template, &migration::schema_changes())?;
    let tx = conn.transaction().map_err(db_error)?;
    for (_, sql) in &blueprint.tables {
        search_storage::execute_schema(&tx, sql)?;
    }
    // Empty-library defaults (identity sequence and feature/meta records) are copied
    // explicitly. No schema-5 file is ever published or opened by a writer.
    for (table, _) in &blueprint.tables {
        let columns = tx
            .prepare("SELECT name FROM pragma_table_xinfo(?1) WHERE hidden=0 ORDER BY cid")
            .map_err(db_error)?
            .query_map([table], |row| row.get::<_, String>(0))
            .map_err(db_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_error)?;
        let quote = search_storage::quote_identifier(table);
        let selected = columns
            .iter()
            .map(|column| search_storage::quote_identifier(column))
            .collect::<Vec<_>>()
            .join(",");
        let mut source = template
            .prepare(&format!("SELECT {selected} FROM {quote}"))
            .map_err(db_error)?;
        let mut rows = source.query([]).map_err(db_error)?;
        let placeholders = std::iter::repeat_n("?", columns.len())
            .collect::<Vec<_>>()
            .join(",");
        while let Some(row) = rows.next().map_err(db_error)? {
            let values = (0..columns.len())
                .map(|index| row.get::<_, Value>(index))
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(db_error)?;
            tx.execute(
                &format!("INSERT INTO {quote}({selected}) VALUES({placeholders})"),
                params_from_iter(values),
            )
            .map_err(db_error)?;
        }
    }
    for (_, sql) in &blueprint.objects {
        search_storage::execute_schema(&tx, sql)?;
    }
    search_storage::create_index(&tx)?;
    search_storage::install_triggers(&tx)?;
    if !source_tags_ready(&tx)? {
        tx.execute(
            "UPDATE audio_files SET source_tags_text=audiolib_source_tags_key(metadata_json)",
            [],
        )
        .map_err(db_error)?;
    }
    mark_current(&tx)?;
    tx.commit().map_err(db_error)
}

pub(super) fn rebuild_search(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction().map_err(db_error)?;
    tx.execute_batch(
        "DROP TRIGGER IF EXISTS audio_files_ai; DROP TRIGGER IF EXISTS audio_files_au;
        DROP TRIGGER IF EXISTS audio_files_ad; DROP TABLE IF EXISTS audio_search;",
    )
    .map_err(db_error)?;
    search_storage::create_index(&tx)?;
    let mut after = None;
    loop {
        let (next, count) = search_storage::index_batch(&tx, after, 5000)?;
        if count == 0 {
            break;
        }
        after = next;
    }
    search_storage::install_triggers(&tx)?;
    if !source_tags_ready(&tx)? {
        tx.execute(
            "UPDATE audio_files SET source_tags_text=audiolib_source_tags_key(metadata_json)",
            [],
        )
        .map_err(db_error)?;
    }
    mark_current(&tx)?;
    tx.commit().map_err(db_error)
}
