// crates/audiolib-db/src/schema/current.rs
//! Schema 6 entry points, separate from the retained v1..v5 normalization code.
use super::*;
use crate::{migration, search_storage};
use audiolib_core::compatibility::LibraryFormat;
use rusqlite::{params, params_from_iter, types::Value};
use std::collections::BTreeMap;

pub(crate) fn current_format_ready(conn: &Connection) -> Result<bool> {
    ensure_ready(conn)?;
    validate_supported_feature_revisions(conn)?;
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_error)?;
    if version != SCHEMA_VERSION || !feature_table_exists(conn)? {
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
    if features != LibraryFormat::target().features
        || !source_tags_ready(conn)?
        || !source_search_marked(conn)?
        || !search_storage::structure_matches(conn)?
        || search_storage::columns(conn)?
            .iter()
            .any(|column| column == "play_count")
    {
        return Ok(false);
    }
    let history: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='play_history')",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if history {
        return Ok(false);
    }
    definition::required_match(conn)
}

pub(crate) fn needs_upgrade(conn: &Connection) -> Result<bool> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_error)?;
    if version > SCHEMA_VERSION {
        return Err(Error::Database(format!(
            "Unsupported future schema {version}"
        )));
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
    let mut legacy = Connection::open_in_memory().map_err(db_error)?;
    register_tokenizer(&legacy, crate::load_tokenizer())?;
    migrate(&mut legacy)?;
    let blueprint = search_storage::blueprint_with(&legacy, &migration::schema_changes())?;
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
        let mut source = legacy
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
